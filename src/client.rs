//! Viewer side: connects to a host, reassembles frames, decodes and displays them.

use crate::{decoder, input, proto, viewer};
use anyhow::{Result, bail};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub struct Options {
    pub seconds: Option<u64>,
    /// Save the window contents to viewer.bmp after 200 frames (debug aid).
    pub screenshot: bool,
    /// Test aid: randomly discard this percentage of incoming video datagrams.
    pub drop_percent: u32,
    /// Test aid: instead of the keyboard and mouse, play a fixed input sequence aimed at `remote-desk probe`.
    pub script: bool,
}

/// Window pixels -> 0..=65535 across the whole remote screen.
fn normalize(x: i32, y: i32, w: u32, h: u32) -> (u16, u16) {
    let scale = |v: i32, len: u32| (v.clamp(0, len as i32 - 1) as u64 * 65535 / (len as u64 - 1).max(1)) as u16;
    (scale(x, w), scale(y, h))
}

fn translate(raw: viewer::RawInput, sender: &mut input::Sender, pending_move: &mut Option<(u16, u16)>, w: u32, h: u32) {
    use viewer::RawInput::*;
    match raw {
        Move { x, y } => *pending_move = Some(normalize(x, y, w, h)),
        Button { button, down, x, y } => {
            let (nx, ny) = normalize(x, y, w, h);
            sender.push(if down { proto::ev::BUTTON_DOWN } else { proto::ev::BUTTON_UP }, 0, button as i16, nx, ny);
        }
        Key { scan, extended, down } => {
            sender.push(if down { proto::ev::KEY_DOWN } else { proto::ev::KEY_UP }, extended as u8, scan as i16, 0, 0)
        }
        Wheel { delta, horizontal } => {
            sender.push(if horizontal { proto::ev::HWHEEL } else { proto::ev::WHEEL }, 0, delta, 0, 0)
        }
        ReleaseAll => sender.push(proto::ev::RELEASE_ALL, 0, 0, 0, 0),
    }
}

/// Timed input sequence for `--script`. Targets the probe window: client area 400x300 at screen (100,100).
fn script_steps(info: &proto::StreamInfo, win_w: u32, win_h: u32) -> Vec<(Duration, viewer::RawInput)> {
    use viewer::RawInput::*;
    // Screen pixel -> window pixel, so the normal translate() path maps it back onto the screen.
    let px = |sx: i32, sy: i32| {
        (sx * (win_w as i32 - 1) / (info.width as i32 - 1), sy * (win_h as i32 - 1) / (info.height as i32 - 1))
    };
    let at = |secs: f64| Duration::from_secs_f64(secs);
    let (cx, cy) = px(300, 250);
    let (dx, dy) = px(320, 260);
    let (ex, ey) = px(380, 290);
    vec![
        (at(1.0), Move { x: cx, y: cy }),
        (at(1.3), Button { button: 0, down: true, x: cx, y: cy }),
        (at(1.35), Button { button: 0, down: false, x: cx, y: cy }),
        (at(1.7), Button { button: 1, down: true, x: cx, y: cy }),
        (at(1.75), Button { button: 1, down: false, x: cx, y: cy }),
        (at(2.1), Wheel { delta: 120, horizontal: false }),
        (at(2.2), Wheel { delta: -120, horizontal: false }),
        (at(2.5), Key { scan: 0x1E, extended: false, down: true }), // A
        (at(2.55), Key { scan: 0x1E, extended: false, down: false }),
        (at(2.6), Key { scan: 0x4D, extended: true, down: true }), // Right arrow
        (at(2.65), Key { scan: 0x4D, extended: true, down: false }),
        (at(3.0), Button { button: 0, down: true, x: dx, y: dy }), // drag from (320,260) to (380,290)
        (at(3.1), Move { x: ex, y: ey }),
        (at(3.2), Button { button: 0, down: false, x: ex, y: ey }),
    ]
}

pub fn run(host: SocketAddr, opts: Options) -> Result<()> {
    let sock = proto::bind(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    sock.connect(host)?;

    // --- handshake: repeat HELLO until the host (and its user) accepts
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    println!("Connecting to {host} - waiting for the host to approve...");
    let started = Instant::now();
    let mut buf = [0u8; 2048];
    let mut port_closed = false; // ICMP "port unreachable" seen: the PC answers but nothing listens on the port
    let mut last_hint = Instant::now();
    let info = loop {
        if started.elapsed() > Duration::from_secs(60) {
            bail!("host did not answer within 60 s");
        }
        let _ = sock.send(&[proto::HELLO, proto::VERSION]);
        match sock.recv(&mut buf) {
            Ok(_) if buf[0] == proto::BYE => bail!("host declined the connection"),
            Ok(n) => {
                if let Some(info) = proto::StreamInfo::decode(&buf[..n]) {
                    break info;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                port_closed = true;
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) if proto::is_transient(&e) => {} // plain timeout: the 500 ms read timeout paces the retries
            Err(e) => return Err(e.into()),
        }
        if last_hint.elapsed() >= Duration::from_secs(5) {
            last_hint = Instant::now();
            if port_closed {
                println!(
                    "  {host} is reachable, but no remote-desk host is listening on that port. \
                     Start 2-pokreni-host.bat on that PC and check that the IP and port are right."
                );
            } else {
                println!(
                    "  No answer from {host} yet. Check: (1) the host is running and prints exactly this IP, \
                     (2) both PCs are on the same network, (3) 1-dozvoli-firewall.bat was run as administrator on the host, \
                     (4) the host window is not paused - click it and press Esc."
                );
            }
        }
    };
    println!(
        "Connected: {}x{} @ {} fps, {}",
        info.width,
        info.height,
        info.fps,
        if info.control { "remote control allowed" } else { "view only" }
    );

    // --- local GPU objects
    let (device, context) = viewer::create_device()?;
    let win_w = 1280;
    let win_h = (win_w as u64 * info.height as u64 / info.width as u64) as u32;
    let mut window = viewer::Viewer::new(&device, &context, info.width, info.height, win_w, win_h)?;
    let mut dec = decoder::Decoder::new(&device)?;

    // --- receive thread: sockets -> reassembled frames
    // Short timeout: the receive loop doubles as the timer for NACK and give-up decisions.
    sock.set_read_timeout(Some(Duration::from_millis(2)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let rtt_us = Arc::new(AtomicU64::new(0));
    let rx_bytes = Arc::new(AtomicU64::new(0));
    let nacks_sent = Arc::new(AtomicU64::new(0));
    let input_ack = Arc::new(AtomicU32::new(0));
    let clock_offset_us = Arc::new(AtomicI64::new(0));
    let (tx, rx) = mpsc::channel::<proto::Assembled>();
    let rx_thread = {
        let sock = sock.try_clone()?;
        let (stop, rtt_us, rx_bytes, nacks_sent) = (stop.clone(), rtt_us.clone(), rx_bytes.clone(), nacks_sent.clone());
        let input_ack = input_ack.clone();
        let clock_offset_us = clock_offset_us.clone();
        let drop_percent = opts.drop_percent as u64;
        std::thread::spawn(move || {
            let mut reasm = proto::Reassembler::default();
            let mut rng = 0x9E37_79B9_7F4A_7C15u64;
            let mut best_rtt_us = u64::MAX / 2;
            let mut buf = vec![0u8; 2048];
            while !stop.load(Ordering::Relaxed) {
                match sock.recv(&mut buf) {
                    Ok(n) if n > 0 => match buf[0] {
                        proto::VIDEO => {
                            rx_bytes.fetch_add(n as u64, Ordering::Relaxed);
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            let simulated_loss = drop_percent > 0 && rng % 100 < drop_percent;
                            if !simulated_loss {
                                reasm.push(&buf[..n], Instant::now());
                            }
                        }
                        proto::PONG if n >= 21 => {
                            let now = proto::now_us();
                            let sent = u64::from_le_bytes(buf[1..9].try_into().unwrap());
                            let rtt = now.saturating_sub(sent);
                            rtt_us.store(rtt, Ordering::Relaxed);
                            // Clock offset (host minus viewer), NTP style: assume the reply left the host halfway
                            // through the round trip. The sample with the smallest RTT is the most trustworthy.
                            let host_now = u64::from_le_bytes(buf[13..21].try_into().unwrap());
                            if rtt <= best_rtt_us.saturating_add(2_000) {
                                best_rtt_us = best_rtt_us.min(rtt);
                                let offset = host_now as i64 - (sent + rtt / 2) as i64;
                                clock_offset_us.store(offset, Ordering::Relaxed);
                            }
                            let latest = u32::from_le_bytes(buf[9..13].try_into().unwrap());
                            if latest != u32::MAX {
                                reasm.note_latest(latest);
                            }
                        }
                        proto::INPUT_ACK if n >= 5 => {
                            input_ack.store(u32::from_le_bytes(buf[1..5].try_into().unwrap()), Ordering::Relaxed);
                        }
                        proto::BYE => stop.store(true, Ordering::Relaxed),
                        _ => {}
                    },
                    Ok(_) => {}
                    Err(e) if proto::is_transient(&e) => {}
                    Err(_) => break,
                }

                let poll = reasm.poll(Instant::now(), Duration::from_micros(rtt_us.load(Ordering::Relaxed)));
                for batch in poll.nacks.chunks(proto::MAX_NACK_ENTRIES) {
                    let _ = sock.send(&proto::encode_nack(batch));
                    nacks_sent.fetch_add(batch.len() as u64, Ordering::Relaxed);
                }
                for frame in poll.frames {
                    if tx.send(frame).is_err() {
                        return;
                    }
                }
            }
        })
    };

    // --- main loop: decode + present on the thread that owns the window
    let mut need_keyframe = true;
    let mut last_keyframe_req = Instant::now() - Duration::from_secs(1);
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut last_report = Instant::now();
    let (mut shown, mut total_shown, mut lost_frames, mut keyframe_reqs) = (0u32, 0u32, 0u32, 0u32);
    let mut latencies: Vec<u64> = Vec::new();
    let mut last_rx_bytes = 0u64;
    let deadline = opts.seconds.map(Duration::from_secs);

    let mut sender = input::Sender::new();
    let script = if opts.script && info.control { script_steps(&info, win_w, win_h) } else { Vec::new() };
    let mut script_next = 0;
    let session_start = Instant::now();

    while window.pump() && !stop.load(Ordering::Relaxed) && deadline.is_none_or(|d| started.elapsed() < d) {
        // --- remote control: window input (or the test script) -> protocol events
        let mut raw_input = window.take_input();
        while script_next < script.len() && session_start.elapsed() >= script[script_next].0 {
            raw_input.push(script[script_next].1);
            script_next += 1;
        }
        if info.control {
            let mut pending_move = None;
            for raw in raw_input {
                translate(raw, &mut sender, &mut pending_move, win_w, win_h);
            }
            if let Some((x, y)) = pending_move {
                sender.send_move(&sock, x, y);
            }
            sender.ack(input_ack.load(Ordering::Relaxed));
            sender.flush(&sock, Instant::now(), Duration::from_micros(rtt_us.load(Ordering::Relaxed)));
        }

        if last_ping.elapsed() >= Duration::from_millis(500) {
            let mut ping = [0u8; 9];
            ping[0] = proto::PING;
            ping[1..9].copy_from_slice(&proto::now_us().to_le_bytes());
            let _ = sock.send(&ping);
            last_ping = Instant::now();
        }

        // Short wait keeps the added input latency (window events are only read once per iteration) tiny.
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(frame) => {
                if frame.gap {
                    lost_frames += 1;
                    need_keyframe = true;
                }
                if need_keyframe {
                    if !frame.keyframe {
                        // Inter-frames are undecodable until a keyframe arrives; ask for one (rate limited).
                        if last_keyframe_req.elapsed() > Duration::from_millis(100) {
                            let _ = sock.send(&[proto::KEYFRAME_REQ]);
                            last_keyframe_req = Instant::now();
                            keyframe_reqs += 1;
                        }
                        continue;
                    }
                    need_keyframe = false;
                }
                for decoded in dec.decode(&frame.data)? {
                    window.present(&decoded)?;
                    shown += 1;
                    total_shown += 1;
                    // Capture time is on the host's clock; shift it onto ours using the estimated offset.
                    let capture_here = frame.capture_us as i64 - clock_offset_us.load(Ordering::Relaxed);
                    latencies.push((proto::now_us() as i64 - capture_here).max(0) as u64);
                    if opts.screenshot && total_shown == 200 {
                        window.screenshot = Some("viewer.bmp".into());
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_report.elapsed() >= Duration::from_secs(1) {
            let secs = last_report.elapsed().as_secs_f64();
            let total = rx_bytes.load(Ordering::Relaxed);
            latencies.sort_unstable();
            let pct = |p: f64| latencies.get(((latencies.len().max(1) - 1) as f64 * p) as usize).copied().unwrap_or(0);
            println!(
                "{:>3} fps shown | {:>5.1} Mbit/s | capture->present (est.) p50 {:.1} ms p99 {:.1} ms | RTT {:.1} ms | chunks NACKed {} | frames given up {lost_frames} | keyframe req {keyframe_reqs} | input unacked {}",
                (shown as f64 / secs).round(),
                (total - last_rx_bytes) as f64 * 8.0 / secs / 1e6,
                pct(0.5) as f64 / 1000.0,
                pct(0.99) as f64 / 1000.0,
                rtt_us.load(Ordering::Relaxed) as f64 / 1000.0,
                nacks_sent.load(Ordering::Relaxed),
                sender.pending(),
            );
            last_rx_bytes = total;
            shown = 0;
            latencies.clear();
            last_report = Instant::now();
        }
    }

    stop.store(true, Ordering::Relaxed);
    let _ = sock.send(&[proto::BYE]);
    let _ = rx_thread.join();
    Ok(())
}
