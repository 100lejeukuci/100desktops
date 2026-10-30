//! Remote input. The viewer side queues events and resends them until acknowledged (a lost key-up would leave
//! a key stuck); the host side applies them in order through SendInput and remembers what is held down.

use crate::proto::{self, InputEvent, ev};
use std::collections::{HashSet, VecDeque};
use std::net::UdpSocket;
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

// ---------------------------------------------------------------- viewer side

pub struct Sender {
    next_seq: u32,
    unacked: VecDeque<InputEvent>,
    last_sent: Instant,
    dirty: bool,
    move_seq: u32,
    last_move: Option<(u16, u16)>,
}

impl Sender {
    pub fn new() -> Self {
        Self { next_seq: 0, unacked: VecDeque::new(), last_sent: Instant::now(), dirty: false, move_seq: 0, last_move: None }
    }

    pub fn push(&mut self, kind: u8, flags: u8, value: i16, x: u16, y: u16) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.unacked.push_back(InputEvent { seq, kind, flags, value, x, y });
        self.dirty = true;
    }

    /// Events sent but not yet acknowledged by the host.
    pub fn pending(&self) -> usize {
        self.unacked.len()
    }

    /// The host reports the next sequence number it expects; everything before it has been applied.
    pub fn ack(&mut self, next_expected: u32) {
        while self.unacked.front().is_some_and(|e| e.seq < next_expected) {
            self.unacked.pop_front();
        }
    }

    /// Unreliable pointer position; a lost update is simply replaced by the next one.
    pub fn send_move(&mut self, sock: &UdpSocket, x: u16, y: u16) {
        if self.last_move == Some((x, y)) {
            return;
        }
        self.last_move = Some((x, y));
        self.move_seq = self.move_seq.wrapping_add(1);
        let mut b = [0u8; 9];
        b[0] = proto::INPUT_MOVE;
        b[1..5].copy_from_slice(&self.move_seq.to_le_bytes());
        b[5..7].copy_from_slice(&x.to_le_bytes());
        b[7..9].copy_from_slice(&y.to_le_bytes());
        let _ = sock.send(&b);
    }

    /// Send new events immediately and unacknowledged ones again once they are overdue.
    pub fn flush(&mut self, sock: &UdpSocket, now: Instant, rtt: Duration) {
        if self.unacked.is_empty() {
            return;
        }
        let resend_after = (rtt * 3 / 2).max(Duration::from_millis(10));
        if !self.dirty && now.duration_since(self.last_sent) < resend_after {
            return;
        }
        let batch: Vec<&InputEvent> = self.unacked.iter().take(proto::MAX_EVENTS_PER_PACKET).collect();
        let mut out = Vec::with_capacity(2 + batch.len() * proto::EVENT_SIZE);
        out.push(proto::INPUT_EVENTS);
        out.push(batch.len() as u8);
        for e in batch {
            e.write(&mut out);
        }
        let _ = sock.send(&out);
        self.dirty = false;
        self.last_sent = now;
    }
}

// ------------------------------------------------------------------ host side

#[derive(Default)]
pub struct Injector {
    next_seq: u32,
    move_seq: u32,
    keys: HashSet<(u16, bool)>,
    buttons: HashSet<u8>,
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32, x: u16, y: u16) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx: x as i32, dy: y as i32, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn move_to(x: u16, y: u16) -> INPUT {
    mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, 0, x, y)
}

fn key(scan: u16, extended: bool, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

/// Mouse button flags for (down, up) plus the X-button data value.
fn button_flags(button: u8) -> Option<(MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS, i32)> {
    Some(match button {
        0 => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
        1 => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        2 => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        3 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 1),
        4 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 2),
        _ => return None,
    })
}

impl Injector {
    /// Apply the events of one INPUT_EVENTS datagram in order and return the sequence number to acknowledge.
    pub fn handle_events(&mut self, pkt: &[u8]) -> u32 {
        for e in proto::decode_events(pkt) {
            // Duplicates (already applied) are skipped; a gap stops us until the viewer resends the missing event.
            if e.seq == self.next_seq {
                self.apply(&e);
                self.next_seq = self.next_seq.wrapping_add(1);
            }
        }
        self.next_seq
    }

    pub fn handle_move(&mut self, pkt: &[u8]) {
        if pkt.len() < 9 || pkt[0] != proto::INPUT_MOVE {
            return;
        }
        let seq = u32::from_le_bytes([pkt[1], pkt[2], pkt[3], pkt[4]]);
        if seq.wrapping_sub(self.move_seq) as i32 <= 0 && self.move_seq != 0 {
            return; // reordered, older than what we already applied
        }
        self.move_seq = seq;
        send(&[move_to(u16::from_le_bytes([pkt[5], pkt[6]]), u16::from_le_bytes([pkt[7], pkt[8]]))]);
    }

    fn apply(&mut self, e: &InputEvent) {
        match e.kind {
            ev::KEY_DOWN | ev::KEY_UP => {
                let scan = e.value as u16;
                let extended = e.flags & 1 != 0;
                let up = e.kind == ev::KEY_UP;
                if up {
                    // Releasing something we are not holding (e.g. after RELEASE_ALL) would only add noise.
                    if !self.keys.remove(&(scan, extended)) {
                        return;
                    }
                } else {
                    self.keys.insert((scan, extended));
                }
                send(&[key(scan, extended, up)]);
            }
            ev::BUTTON_DOWN | ev::BUTTON_UP => {
                let button = e.value as u8;
                let Some((down, up, data)) = button_flags(button) else { return };
                let is_up = e.kind == ev::BUTTON_UP;
                if is_up {
                    if !self.buttons.remove(&button) {
                        return;
                    }
                } else {
                    self.buttons.insert(button);
                }
                send(&[move_to(e.x, e.y), mouse(if is_up { up } else { down }, data, 0, 0)]);
            }
            ev::WHEEL => send(&[mouse(MOUSEEVENTF_WHEEL, e.value as i32, 0, 0)]),
            ev::HWHEEL => send(&[mouse(MOUSEEVENTF_HWHEEL, e.value as i32, 0, 0)]),
            ev::RELEASE_ALL => self.release_all(),
            _ => {}
        }
    }

    /// Let go of everything the viewer was holding; called when the session ends or the viewer loses focus.
    pub fn release_all(&mut self) {
        for (scan, extended) in self.keys.drain().collect::<Vec<_>>() {
            send(&[key(scan, extended, true)]);
        }
        for button in self.buttons.drain().collect::<Vec<_>>() {
            if let Some((_, up, data)) = button_flags(button) {
                send(&[mouse(up, data, 0, 0)]);
            }
        }
    }
}

/// Emergency stop on the host: Ctrl+Alt+Shift+F12 ends the session no matter what the viewer is doing.
pub fn panic_hotkey_down() -> bool {
    unsafe { [VK_CONTROL, VK_MENU, VK_SHIFT, VK_F12].iter().all(|k| GetAsyncKeyState(k.0 as i32) < 0) }
}
