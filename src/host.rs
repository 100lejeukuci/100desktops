//! Host side: waits for a viewer, asks the local user for consent, then streams the screen.

use crate::log::{console, say};
use crate::status::{self, HostStatus};
use crate::{capture, encoder, input, proto};
use anyhow::Result;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FPS: u32 = 60;
const BITRATE: u32 = 20_000_000;
/// Viewer pings every 500 ms; silence for this long ends the session.
const PEER_TIMEOUT_US: u64 = 3_000_000;

struct Shared {
    want_keyframe: AtomicBool,
    stop: AtomicBool,
    last_seen_us: AtomicU64,
    /// Newest frame id sent, or u32::MAX before the first one; echoed in PONG so the viewer can spot lost tails.
    latest_frame: AtomicU32,
    history: Mutex<proto::History>,
    retransmitted: AtomicU64,
}

/// What the local user allows a viewer to do.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Permission {
    Deny,
    ViewOnly,
    Control,
}

/// Listen on `port` until `stop` is set. Every incoming viewer is put to `ask`, which decides what it may do.
/// `ask` runs on this thread and may block (the GUI shows a dialog); it should return `Deny` once `stop` is set.
pub fn run(port: u16, mut ask: impl FnMut(SocketAddr) -> Permission, stop: &AtomicBool) -> Result<()> {
    let result = listen(port, &mut ask, stop);
    status::set_host(HostStatus::Off);
    result
}

fn listen(port: u16, ask: &mut dyn FnMut(SocketAddr) -> Permission, stop: &AtomicBool) -> Result<()> {
    let sock = proto::bind(SocketAddr::from(([0, 0, 0, 0], port)))
        .map_err(|e| anyhow::anyhow!("ne mogu da otvorim UDP port {port} (zauzet? pokrenut drugi host?): {e}"))?;
    status::set_host(HostStatus::Waiting { port });
    say(format!("Host sluša na UDP portu {port}. Čeka viewer-a..."));
    let mut buf = [0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        // Short timeout so a stop request is noticed quickly.
        sock.set_read_timeout(Some(Duration::from_millis(250)))?;
        let (n, peer) = match sock.recv_from(&mut buf) {
            Ok(r) => r,
            Err(e) if proto::is_transient(&e) => continue,
            Err(e) => return Err(e.into()),
        };
        if n < 2 || buf[0] != proto::HELLO {
            continue;
        }
        say(format!("Zahtev za povezivanje sa {peer}."));
        status::set_host(HostStatus::AwaitingConsent(peer));
        // Tell the viewer its request arrived, so it shows "waiting for approval" instead of "no answer".
        let _ = sock.send_to(&[proto::PENDING], peer);
        let permission = ask(peer);
        if permission == Permission::Deny {
            let _ = sock.send_to(&[proto::BYE], peer);
            say(format!("Odbijeno: {peer}."));
        } else {
            match session(&sock, peer, permission == Permission::Control, stop) {
                Ok(()) => say(format!("Sesija sa {peer} je završena.")),
                Err(e) => say(format!("Sesija sa {peer} nije uspela: {e:#}")),
            }
        }
        // The viewer keeps repeating its HELLO while we are busy; forget those so it is not asked about twice.
        sock.set_nonblocking(true)?;
        while sock.recv_from(&mut buf).is_ok() {}
        sock.set_nonblocking(false)?;
        status::set_host(HostStatus::Waiting { port });
    }
    say("Host je zaustavljen.");
    Ok(())
}

/// Console prompt used by the CLI.
pub fn ask_stdin(peer: SocketAddr) -> Permission {
    println!("\n{peer} wants to connect to this computer.");
    println!("  c = allow viewing AND control of mouse/keyboard");
    println!("  v = allow viewing only");
    println!("  anything else = refuse");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    match line.trim().to_ascii_lowercase().as_str() {
        "c" | "y" => Permission::Control,
        "v" => Permission::ViewOnly,
        _ => Permission::Deny,
    }
}

fn session(sock: &UdpSocket, peer: SocketAddr, allow_control: bool, stop: &AtomicBool) -> Result<()> {
    let cap = capture::Capturer::new()?;
    let tex = cap.frame_texture()?;
    let mut enc = encoder::Encoder::new(&cap.device, &cap.context, &tex, cap.width, cap.height, FPS, BITRATE)?;
    let info = proto::StreamInfo { width: cap.width, height: cap.height, fps: FPS, control: allow_control };
    sock.send_to(&info.encode(), peer)?;
    status::set_host(HostStatus::Streaming { peer, control: allow_control });
    say(format!(
        "Prenosim ekran ({}x{} @ {FPS} fps) ka {peer}: {}",
        cap.width,
        cap.height,
        if allow_control { "gledanje i kontrola (hitni prekid: Ctrl+Alt+Shift+F12)" } else { "samo gledanje" }
    ));

    let shared = Arc::new(Shared {
        want_keyframe: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        last_seen_us: AtomicU64::new(proto::now_us()),
        latest_frame: AtomicU32::new(u32::MAX),
        history: Mutex::new(proto::History::default()),
        retransmitted: AtomicU64::new(0),
    });
    let ctl_sock = sock.try_clone()?;
    ctl_sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let ctl_shared = shared.clone();
    let control = std::thread::spawn(move || control_loop(ctl_sock, peer, info, ctl_shared));

    let mut frame_id = 0u32;
    let mut sent_bytes = 0usize;
    let result = (|| -> Result<()> {
        while !shared.stop.load(Ordering::Relaxed)
            && !stop.load(Ordering::Relaxed)
            && proto::now_us().saturating_sub(shared.last_seen_us.load(Ordering::Relaxed)) < PEER_TIMEOUT_US
        {
            if allow_control && input::panic_hotkey_down() {
                say("Hitni prekid (Ctrl+Alt+Shift+F12): sesija se završava.");
                break;
            }
            let want_key = shared.want_keyframe.swap(false, Ordering::Relaxed);
            if want_key {
                enc.force_keyframe()?;
            }
            let have_frame = match cap.next_frame(&tex, 50)? {
                capture::Frame::New => true,
                // Static screen: a requested keyframe is still produced from the last picture.
                capture::Frame::Unchanged => want_key,
            };
            if !have_frame {
                continue;
            }
            for p in enc.encode()? {
                let captured_us = proto::now_us().saturating_sub(p.latency.as_micros() as u64);
                let datagrams = Arc::new(proto::packetize(frame_id, p.keyframe, captured_us, &p.data));
                shared.history.lock().unwrap().push(frame_id, datagrams.clone());
                for d in datagrams.iter() {
                    sock.send_to(d, peer)?;
                }
                shared.latest_frame.store(frame_id, Ordering::Relaxed);
                frame_id = frame_id.wrapping_add(1);
                sent_bytes += p.data.len();
            }
        }
        Ok(())
    })();

    shared.stop.store(true, Ordering::Relaxed);
    let _ = control.join();
    let _ = sock.send_to(&[proto::BYE], peer);
    console(format!(
        "Sent {frame_id} frames, {:.1} MB, {} chunks resent after NACK",
        sent_bytes as f64 / 1e6,
        shared.retransmitted.load(Ordering::Relaxed)
    ));
    result
}

/// Handles everything the viewer sends back while video is flowing.
fn control_loop(sock: UdpSocket, peer: SocketAddr, info: proto::StreamInfo, shared: Arc<Shared>) {
    let mut buf = [0u8; 1500];
    let mut injector = input::Injector::default();
    while !shared.stop.load(Ordering::Relaxed) {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == peer && n > 0 => {
                shared.last_seen_us.store(proto::now_us(), Ordering::Relaxed);
                match buf[0] {
                    proto::PING if n >= 9 => {
                        let mut pong = [0u8; 21];
                        pong[..9].copy_from_slice(&buf[..9]);
                        pong[0] = proto::PONG;
                        pong[9..13].copy_from_slice(&shared.latest_frame.load(Ordering::Relaxed).to_le_bytes());
                        pong[13..21].copy_from_slice(&proto::now_us().to_le_bytes());
                        let _ = sock.send_to(&pong, peer);
                    }
                    proto::NACK => {
                        for (frame, chunk) in proto::decode_nack(&buf[..n]) {
                            let Some(datagrams) = shared.history.lock().unwrap().get(frame) else { continue };
                            let wanted = if chunk == proto::WHOLE_FRAME {
                                &datagrams[..]
                            } else {
                                datagrams.get(chunk as usize..chunk as usize + 1).unwrap_or(&[])
                            };
                            for d in wanted {
                                let _ = sock.send_to(d, peer);
                                shared.retransmitted.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    // Input is only ever applied when the local user granted control for this session.
                    proto::INPUT_MOVE if info.control => injector.handle_move(&buf[..n]),
                    proto::INPUT_EVENTS if info.control => {
                        let next_expected = injector.handle_events(&buf[..n]);
                        let mut ack = [0u8; 5];
                        ack[0] = proto::INPUT_ACK;
                        ack[1..5].copy_from_slice(&next_expected.to_le_bytes());
                        let _ = sock.send_to(&ack, peer);
                    }
                    proto::KEYFRAME_REQ => shared.want_keyframe.store(true, Ordering::Relaxed),
                    proto::HELLO => {
                        let _ = sock.send_to(&info.encode(), peer);
                    }
                    proto::BYE => shared.stop.store(true, Ordering::Relaxed),
                    _ => {}
                }
            }
            Ok(_) => {} // someone else; only the approved viewer may talk to us
            Err(e) if proto::is_transient(&e) => {}
            Err(_) => break,
        }
    }
    // Never leave a key or mouse button held down on this machine after the session.
    injector.release_all();
}
