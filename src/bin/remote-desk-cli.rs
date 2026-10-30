//! Command-line version of remote-desk, used for tests and scripting. The everyday app is `remote-desk.exe` (GUI).

use anyhow::{Context, Result, bail};
use remote_desk::{client, host, local, log, probe};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::AtomicBool;

const USAGE: &str = "\
Usage:
  remote-desk-cli local                              single-PC benchmark (no network)
  remote-desk-cli host [--port 7000]                 share this screen; asks whether to allow view + control, view only, or refuse
                       [--yes [--view-only]]         answer that question automatically (tests only)
  remote-desk-cli view <host:port> [--seconds N] [--shot] [--drop PERCENT] [--script]
  remote-desk-cli probe [--seconds N]                test window that prints the input it receives
";

/// A click in a console window starts "QuickEdit" text selection, which freezes any program that prints
/// until a key is pressed. That would stall the host's consent prompt and the viewer's video loop.
fn disable_quick_edit() {
    use windows::Win32::System::Console::*;
    unsafe {
        if let Ok(input) = GetStdHandle(STD_INPUT_HANDLE) {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(input, &mut mode).is_ok() {
                let _ = SetConsoleMode(input, (mode & !ENABLE_QUICK_EDIT_MODE) | ENABLE_EXTENDED_FLAGS);
            }
        }
    }
}

fn main() -> Result<()> {
    disable_quick_edit();
    log::enable_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let never_stop = AtomicBool::new(false);
    match args.first().map(String::as_str) {
        Some("local") => local::run(),
        Some("host") => {
            let port = value("--port").map(|p| p.parse()).transpose().context("bad --port")?.unwrap_or(7000);
            let auto = flag("--yes").then(|| {
                if flag("--view-only") { host::Permission::ViewOnly } else { host::Permission::Control }
            });
            host::run(port, |peer| auto.unwrap_or_else(|| host::ask_stdin(peer)), &never_stop)
        }
        Some("view") => {
            let target = args.get(1).context("missing host:port")?;
            let addr: SocketAddr = target
                .to_socket_addrs()
                .context("bad host:port")?
                .find(SocketAddr::is_ipv4)
                .context("no IPv4 address for host")?;
            let seconds = value("--seconds").map(|s| s.parse()).transpose().context("bad --seconds")?;
            let drop_percent = value("--drop").map(|s| s.parse()).transpose().context("bad --drop")?.unwrap_or(0);
            client::run(
                addr,
                client::Options { seconds, screenshot: flag("--shot"), drop_percent, script: flag("--script") },
                &never_stop,
            )
        }
        Some("probe") => probe::run(value("--seconds").map(|s| s.parse()).transpose().context("bad --seconds")?.unwrap_or(15)),
        _ => bail!("{USAGE}"),
    }
}
