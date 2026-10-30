//! One place for user-visible messages. The GUI shows the event log; the CLI also prints it to the console.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_LINES: usize = 300;

static CONSOLE: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Print messages to stdout as well (CLI mode). The GUI build has no console.
pub fn enable_console() {
    CONSOLE.store(true, Ordering::Relaxed);
}

fn timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

/// An event the user should see: connections, refusals, errors.
pub fn say(msg: impl AsRef<str>) {
    let line = format!("[{}] {}", timestamp(), msg.as_ref());
    if CONSOLE.load(Ordering::Relaxed) {
        println!("{line}");
    }
    if let Ok(mut log) = LOG.lock() {
        if log.len() >= MAX_LINES {
            log.pop_front();
        }
        log.push_back(line);
    }
}

/// Chatty output (per-second statistics): console only, never the GUI log.
pub fn console(msg: impl AsRef<str>) {
    if CONSOLE.load(Ordering::Relaxed) {
        println!("{}", msg.as_ref());
    }
}

pub fn lines() -> Vec<String> {
    LOG.lock().map(|l| l.iter().cloned().collect()).unwrap_or_default()
}
