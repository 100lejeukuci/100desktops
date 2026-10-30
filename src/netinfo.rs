//! This computer's IPv4 addresses, for showing "your address" in the GUI.

use std::net::{Ipv4Addr, UdpSocket};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn usable(ip: &Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified()
}

/// The address the OS would use to reach the internet/LAN. Connecting a UDP socket sends nothing.
pub fn primary_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if usable(&ip) => Some(ip),
        _ => None,
    }
}

/// Every usable IPv4 address of this PC (from `ipconfig`, which works whatever the Windows language is).
pub fn local_ips() -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    if let Ok(out) = Command::new("ipconfig").creation_flags(CREATE_NO_WINDOW).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if !line.contains("IPv4") {
                continue;
            }
            let ip = line
                .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                .filter_map(|tok| tok.parse::<Ipv4Addr>().ok())
                .find(usable);
            if let Some(ip) = ip {
                if !found.contains(&ip) {
                    found.push(ip);
                }
            }
        }
    }
    if let Some(primary) = primary_ip() {
        // Primary first; also covers the case where ipconfig could not be parsed.
        found.retain(|ip| *ip != primary);
        found.insert(0, primary);
    }
    found
}
