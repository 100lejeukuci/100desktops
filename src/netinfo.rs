//! This computer's IPv4 addresses, for showing "your address" in the GUI.

use std::net::{Ipv4Addr, UdpSocket};
use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn usable(ip: &Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified()
}

/// Tailscale hands out addresses from the carrier-grade NAT block 100.64.0.0/10.
pub fn is_tailscale(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Addresses that only mean something inside one local network (and are not Tailscale's).
pub fn is_lan_private(ip: &Ipv4Addr) -> bool {
    ip.is_private() && !is_tailscale(ip)
}

/// True when `target` is a private LAN address that none of our own LAN addresses can reach directly
/// (different /24, the usual home-router subnet; a soft warning only), i.e. it will not work from a computer on another network.
pub fn unreachable_lan_target(target: &Ipv4Addr, ours: &[Ipv4Addr]) -> bool {
    if !is_lan_private(target) {
        return false;
    }
    let t = target.octets();
    !ours.iter().any(|ip| is_lan_private(ip) && ip.octets()[..3] == t[..3])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn tailscale_range() {
        assert!(is_tailscale(&ip("100.64.0.1")));
        assert!(is_tailscale(&ip("100.101.102.103")));
        assert!(is_tailscale(&ip("100.127.255.255")));
        assert!(!is_tailscale(&ip("100.128.0.1")));
        assert!(!is_tailscale(&ip("100.63.0.1")));
        assert!(!is_tailscale(&ip("192.168.1.1")));
    }

    #[test]
    fn lan_target_from_another_network() {
        let ours = [ip("192.168.1.20"), ip("100.90.1.2")];
        assert!(unreachable_lan_target(&ip("192.168.100.72"), &ours));
        assert!(!unreachable_lan_target(&ip("192.168.1.55"), &ours));
        assert!(!unreachable_lan_target(&ip("100.88.7.7"), &ours)); // Tailscale target is fine
        assert!(!unreachable_lan_target(&ip("176.67.194.67"), &ours)); // public address is fine
    }
}
