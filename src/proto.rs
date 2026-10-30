//! Wire protocol: plain UDP datagrams, video frames split into MTU-sized chunks.
//!
//! Loss repair, in order of preference:
//!   1. NACK: the viewer names the chunks it is missing and the host resends them from a short history.
//!   2. Keyframe request: if a frame cannot be repaired in time, the viewer drops it and asks for a fresh keyframe.

use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HELLO: u8 = 1; // viewer -> host: [HELLO, version]
pub const STREAM_INFO: u8 = 2; // host -> viewer: [STREAM_INFO, width u32, height u32, fps u32]
pub const VIDEO: u8 = 3; // host -> viewer: header below + payload
pub const KEYFRAME_REQ: u8 = 4; // viewer -> host
pub const PING: u8 = 5; // viewer -> host: [PING, sent_us u64]
pub const PONG: u8 = 6; // host -> viewer: [PONG, sent_us u64, latest_frame_id u32, host_now_us u64] (id = u32::MAX before the first frame)
pub const BYE: u8 = 7; // either direction
pub const NACK: u8 = 8; // viewer -> host: [NACK, n u16, (frame_id u32, chunk u16) * n]; chunk 0xFFFF = whole frame

pub const INPUT_MOVE: u8 = 9; // viewer -> host, unreliable: [INPUT_MOVE, seq u32, x u16, y u16] (0..=65535 over the screen)
pub const INPUT_EVENTS: u8 = 10; // viewer -> host, reliable: [INPUT_EVENTS, n u8, InputEvent * n]
pub const INPUT_ACK: u8 = 11; // host -> viewer: [INPUT_ACK, next_expected_seq u32]

pub const PENDING: u8 = 12; // host -> viewer: request received, the local user has not answered yet

pub const VERSION: u8 = 1;
pub const EVENT_SIZE: usize = 12;
pub const MAX_EVENTS_PER_PACKET: usize = 80;
/// Payload bytes per datagram; leaves headroom for IP/UDP headers inside a 1500-byte Ethernet MTU.
pub const MTU_PAYLOAD: usize = 1200;
/// VIDEO header: type u8, frame_id u32, chunk_index u16, chunk_count u16, flags u8 (bit0 = keyframe), capture_us u64.
pub const VIDEO_HEADER: usize = 18;
pub const WHOLE_FRAME: u16 = 0xFFFF;
pub const MAX_NACK_ENTRIES: usize = 150;
/// Frames the host keeps around for retransmission (about 3 s at 60 fps).
const HISTORY_FRAMES: usize = 180;
const NACK_MAX_TRIES: u8 = 6;
/// A frame that stopped receiving chunks for this long is assumed to be missing its tail.
const STALE_AFTER: Duration = Duration::from_millis(4);

pub fn now_us() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

pub fn bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // A keyframe is hundreds of datagrams arriving back to back; the default buffer would drop them.
    s.set_recv_buffer_size(8 << 20)?;
    s.set_send_buffer_size(8 << 20)?;
    s.bind(&addr.into())?;
    Ok(s.into())
}

/// Windows reports ICMP "port unreachable" from an earlier send as an error on the next receive.
pub fn is_transient(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(e.kind(), WouldBlock | TimedOut | ConnectionReset | Interrupted)
}

#[derive(Clone, Copy, Debug)]
pub struct StreamInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// The host user allowed remote control (otherwise view only).
    pub control: bool,
}

impl StreamInfo {
    pub fn encode(&self) -> [u8; 14] {
        let mut b = [0u8; 14];
        b[0] = STREAM_INFO;
        b[1..5].copy_from_slice(&self.width.to_le_bytes());
        b[5..9].copy_from_slice(&self.height.to_le_bytes());
        b[9..13].copy_from_slice(&self.fps.to_le_bytes());
        b[13] = self.control as u8;
        b
    }

    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < 14 || b[0] != STREAM_INFO {
            return None;
        }
        Some(Self {
            width: u32::from_le_bytes(b[1..5].try_into().ok()?),
            height: u32::from_le_bytes(b[5..9].try_into().ok()?),
            fps: u32::from_le_bytes(b[9..13].try_into().ok()?),
            control: b[13] != 0,
        })
    }
}

/// `InputEvent::kind` values.
pub mod ev {
    pub const KEY_DOWN: u8 = 1; // value = scancode, flags bit0 = extended key
    pub const KEY_UP: u8 = 2;
    pub const BUTTON_DOWN: u8 = 3; // value = button (0 left, 1 right, 2 middle, 3 X1, 4 X2), x/y = pointer
    pub const BUTTON_UP: u8 = 4;
    pub const WHEEL: u8 = 5; // value = delta (120 = one notch)
    pub const HWHEEL: u8 = 6;
    pub const RELEASE_ALL: u8 = 7;
}

/// One reliable input event, 12 bytes on the wire: seq u32, kind u8, flags u8, value i16, x u16, y u16.
#[derive(Clone, Copy, Debug)]
pub struct InputEvent {
    pub seq: u32,
    pub kind: u8,
    pub flags: u8,
    pub value: i16,
    pub x: u16,
    pub y: u16,
}

impl InputEvent {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.push(self.kind);
        out.push(self.flags);
        out.extend_from_slice(&self.value.to_le_bytes());
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
    }

    pub fn read(b: &[u8]) -> Self {
        Self {
            seq: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            kind: b[4],
            flags: b[5],
            value: i16::from_le_bytes([b[6], b[7]]),
            x: u16::from_le_bytes([b[8], b[9]]),
            y: u16::from_le_bytes([b[10], b[11]]),
        }
    }
}

/// Parse an INPUT_EVENTS datagram.
pub fn decode_events(b: &[u8]) -> Vec<InputEvent> {
    if b.len() < 2 || b[0] != INPUT_EVENTS {
        return Vec::new();
    }
    b[2..].chunks_exact(EVENT_SIZE).take(b[1] as usize).map(InputEvent::read).collect()
}

pub fn encode_nack(entries: &[(u32, u16)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(3 + entries.len() * 6);
    b.push(NACK);
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (frame, chunk) in entries {
        b.extend_from_slice(&frame.to_le_bytes());
        b.extend_from_slice(&chunk.to_le_bytes());
    }
    b
}

pub fn decode_nack(b: &[u8]) -> Vec<(u32, u16)> {
    if b.len() < 3 || b[0] != NACK {
        return Vec::new();
    }
    let n = u16::from_le_bytes([b[1], b[2]]) as usize;
    b[3..]
        .chunks_exact(6)
        .take(n)
        .map(|e| (u32::from_le_bytes([e[0], e[1], e[2], e[3]]), u16::from_le_bytes([e[4], e[5]])))
        .collect()
}

/// Split one encoded frame into ready-to-send datagrams.
pub fn packetize(frame_id: u32, keyframe: bool, capture_us: u64, data: &[u8]) -> Vec<Vec<u8>> {
    let count = data.len().div_ceil(MTU_PAYLOAD).max(1) as u16;
    data.chunks(MTU_PAYLOAD)
        .enumerate()
        .map(|(i, chunk)| {
            let mut d = Vec::with_capacity(VIDEO_HEADER + chunk.len());
            d.push(VIDEO);
            d.extend_from_slice(&frame_id.to_le_bytes());
            d.extend_from_slice(&(i as u16).to_le_bytes());
            d.extend_from_slice(&count.to_le_bytes());
            d.push(keyframe as u8);
            d.extend_from_slice(&capture_us.to_le_bytes());
            d.extend_from_slice(chunk);
            d
        })
        .collect()
}

/// Host-side record of recently sent frames so lost chunks can be resent.
#[derive(Default)]
pub struct History {
    frames: VecDeque<(u32, Arc<Vec<Vec<u8>>>)>,
}

impl History {
    pub fn push(&mut self, frame_id: u32, datagrams: Arc<Vec<Vec<u8>>>) {
        self.frames.push_back((frame_id, datagrams));
        while self.frames.len() > HISTORY_FRAMES {
            self.frames.pop_front();
        }
    }

    pub fn get(&self, frame_id: u32) -> Option<Arc<Vec<Vec<u8>>>> {
        let first = self.frames.front()?.0;
        let index = frame_id.checked_sub(first)? as usize;
        self.frames.get(index).filter(|(id, _)| *id == frame_id).map(|(_, d)| d.clone())
    }
}

pub struct Assembled {
    pub keyframe: bool,
    pub capture_us: u64,
    pub data: Vec<u8>,
    /// Frames before this one were given up on; the decoder state is no longer trustworthy.
    pub gap: bool,
}

struct Partial {
    count: usize,
    got: usize,
    keyframe: bool,
    capture_us: u64,
    data: Vec<u8>,
    seen: Vec<bool>,
    nacks: Vec<(Option<Instant>, u8)>,
    total_len: usize,
    max_idx: usize,
    last_chunk_at: Instant,
}

pub struct Poll {
    pub frames: Vec<Assembled>,
    pub nacks: Vec<(u32, u16)>,
}

/// Rebuilds frames from chunks, delivers them strictly in order, and works out what to NACK.
#[derive(Default)]
pub struct Reassembler {
    partial: BTreeMap<u32, Partial>,
    complete: BTreeMap<u32, Assembled>,
    /// Frames of which no chunk was ever seen: id -> (last NACK time, tries).
    missing: HashMap<u32, (Option<Instant>, u8)>,
    /// Next frame id to hand out. The host numbers frames from 0 for each session.
    next: u32,
    newest: Option<u32>,
    stalled_since: Option<Instant>,
    pending_gap: bool,
}

impl Reassembler {
    /// The host reports the newest frame it sent, which reveals lost frames at the end of a burst.
    pub fn note_latest(&mut self, frame_id: u32) {
        self.newest = Some(self.newest.map_or(frame_id, |n| n.max(frame_id)));
    }

    pub fn push(&mut self, pkt: &[u8], now: Instant) {
        if pkt.len() < VIDEO_HEADER || pkt[0] != VIDEO {
            return;
        }
        let frame_id = u32::from_le_bytes(pkt[1..5].try_into().unwrap());
        let idx = u16::from_le_bytes(pkt[5..7].try_into().unwrap()) as usize;
        let count = u16::from_le_bytes(pkt[7..9].try_into().unwrap()) as usize;
        let keyframe = pkt[9] & 1 != 0;
        let capture_us = u64::from_le_bytes(pkt[10..18].try_into().unwrap());
        let payload = &pkt[VIDEO_HEADER..];
        if count == 0 || idx >= count || payload.len() > MTU_PAYLOAD || frame_id < self.next {
            return;
        }
        self.note_latest(frame_id);
        if self.complete.contains_key(&frame_id) {
            return;
        }
        let p = self.partial.entry(frame_id).or_insert_with(|| Partial {
            count,
            got: 0,
            keyframe,
            capture_us,
            data: vec![0; count * MTU_PAYLOAD],
            seen: vec![false; count],
            nacks: vec![(None, 0); count],
            total_len: 0,
            max_idx: 0,
            last_chunk_at: now,
        });
        if p.count != count || p.seen[idx] {
            return;
        }
        p.seen[idx] = true;
        p.got += 1;
        p.max_idx = p.max_idx.max(idx);
        p.last_chunk_at = now;
        p.data[idx * MTU_PAYLOAD..idx * MTU_PAYLOAD + payload.len()].copy_from_slice(payload);
        if idx == count - 1 {
            p.total_len = idx * MTU_PAYLOAD + payload.len();
        }
        if p.got == count {
            let mut p = self.partial.remove(&frame_id).unwrap();
            p.data.truncate(p.total_len);
            self.missing.remove(&frame_id);
            self.complete.insert(
                frame_id,
                Assembled { keyframe: p.keyframe, capture_us: p.capture_us, data: p.data, gap: false },
            );
        }
    }

    /// Deliver whatever is ready in order, give up on frames that took too long, and list chunks to NACK.
    pub fn poll(&mut self, now: Instant, rtt: Duration) -> Poll {
        let nack_interval = (rtt * 3 / 2).max(Duration::from_millis(8));
        let max_wait = (rtt * 3 + Duration::from_millis(15)).clamp(Duration::from_millis(30), Duration::from_millis(250));
        let mut frames = Vec::new();

        loop {
            while let Some(mut a) = self.complete.remove(&self.next) {
                a.gap = std::mem::take(&mut self.pending_gap);
                frames.push(a);
                self.next = self.next.wrapping_add(1);
                self.stalled_since = None;
            }
            let Some(newest) = self.newest else { break };
            let blocked = newest > self.next
                || self.partial.get(&self.next).is_some_and(|p| now.duration_since(p.last_chunk_at) > STALE_AFTER);
            if !blocked {
                self.stalled_since = None;
                break;
            }
            let since = *self.stalled_since.get_or_insert(now);
            if now.duration_since(since) < max_wait {
                break;
            }
            // Repair did not finish in time: skip to the oldest frame we still have anything of.
            let next = self.next;
            let target = self
                .complete
                .keys()
                .chain(self.partial.keys())
                .filter(|&&k| k > next)
                .min()
                .copied()
                .unwrap_or(newest.wrapping_add(1));
            self.next = target;
            self.pending_gap = true;
            self.stalled_since = None;
            self.partial.retain(|&k, _| k >= target);
        }
        self.missing.retain(|&k, _| k >= self.next);

        let mut nacks = Vec::new();
        let due = |state: &mut (Option<Instant>, u8)| -> bool {
            let ready = state.1 < NACK_MAX_TRIES && state.0.is_none_or(|t| now.duration_since(t) >= nack_interval);
            if ready {
                *state = (Some(now), state.1 + 1);
            }
            ready
        };
        if let Some(newest) = self.newest {
            // Frames of which not a single chunk arrived.
            for id in self.next..=newest.min(self.next.saturating_add(64)) {
                if !self.partial.contains_key(&id) && !self.complete.contains_key(&id) {
                    let state = self.missing.entry(id).or_insert((None, 0));
                    if due(state) {
                        nacks.push((id, WHOLE_FRAME));
                    }
                }
            }
            // Holes inside frames that did start arriving.
            for (&id, p) in self.partial.iter_mut() {
                let tail_lost = id < newest || now.duration_since(p.last_chunk_at) > STALE_AFTER;
                let limit = if tail_lost { p.count } else { p.max_idx };
                for idx in 0..limit {
                    if !p.seen[idx] && due(&mut p.nacks[idx]) {
                        nacks.push((id, idx as u16));
                    }
                }
            }
        }
        Poll { frames, nacks }
    }
}
