mod capture;
mod client;
mod decoder;
mod encoder;
mod host;
mod input;
mod local;
mod probe;
mod proto;
mod viewer;

use anyhow::{Context, Result, bail};
use std::net::{SocketAddr, ToSocketAddrs};

const USAGE: &str = "\
Usage:
  remote-desk local                              single-PC benchmark (no network)
  remote-desk host [--port 7000]                 share this screen; asks whether to allow view + control, view only, or refuse
                   [--yes [--view-only]]         answer that question automatically (tests only)
  remote-desk view <host:port> [--seconds N] [--shot] [--drop PERCENT]
  remote-desk probe [--seconds N]                test window that prints the input it receives
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    match args.first().map(String::as_str) {
        Some("local") => local::run(),
        Some("host") => {
            let port = value("--port").map(|p| p.parse()).transpose().context("bad --port")?.unwrap_or(7000);
            let auto = flag("--yes").then(|| {
                if flag("--view-only") { host::Permission::ViewOnly } else { host::Permission::Control }
            });
            host::run(port, auto)
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
            )
        }
        Some("probe") => probe::run(value("--seconds").map(|s| s.parse()).transpose().context("bad --seconds")?.unwrap_or(15)),
        _ => bail!("{USAGE}"),
    }
}
