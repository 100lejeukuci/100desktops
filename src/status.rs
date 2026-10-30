//! Live state of the host and viewer, written by their worker threads and read by the GUI.

use std::net::SocketAddr;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum HostStatus {
    Off,
    Waiting { port: u16 },
    AwaitingConsent(SocketAddr),
    Streaming { peer: SocketAddr, control: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewerStatus {
    Off,
    Connecting { host: SocketAddr, hint: Option<String> },
    Connected { host: SocketAddr, width: u32, height: u32, control: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub fps: f32,
    pub mbps: f32,
    pub latency_p50_ms: f32,
    pub latency_p99_ms: f32,
    pub rtt_ms: f32,
    pub nacks: u64,
    pub frames_given_up: u32,
}

pub const NO_STATS: Stats =
    Stats { fps: 0.0, mbps: 0.0, latency_p50_ms: 0.0, latency_p99_ms: 0.0, rtt_ms: 0.0, nacks: 0, frames_given_up: 0 };

static HOST: Mutex<HostStatus> = Mutex::new(HostStatus::Off);
static VIEWER: Mutex<ViewerStatus> = Mutex::new(ViewerStatus::Off);
static STATS: Mutex<Stats> = Mutex::new(NO_STATS);

pub fn host() -> HostStatus {
    HOST.lock().map(|s| s.clone()).unwrap_or(HostStatus::Off)
}

pub fn set_host(s: HostStatus) {
    if let Ok(mut g) = HOST.lock() {
        *g = s;
    }
}

pub fn viewer() -> ViewerStatus {
    VIEWER.lock().map(|s| s.clone()).unwrap_or(ViewerStatus::Off)
}

pub fn set_viewer(s: ViewerStatus) {
    if let Ok(mut g) = VIEWER.lock() {
        *g = s;
    }
}

pub fn stats() -> Stats {
    STATS.lock().map(|s| *s).unwrap_or(NO_STATS)
}

pub fn set_stats(s: Stats) {
    if let Ok(mut g) = STATS.lock() {
        *g = s;
    }
}
