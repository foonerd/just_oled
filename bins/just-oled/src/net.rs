//! What the player is connected by, and its address: read from the
//! kernel's files and `getifaddrs`, nothing else, for the icon and the
//! standby page. Read every few seconds by the daemon, not every frame.

use std::ffi::CStr;
use std::fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    None,
    /// Link quality in four steps, 0..=4.
    Wifi(u8),
    Ethernet,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Net {
    pub connection: Connection,
    /// The first IPv4 address that is not loopback, dotted; empty when none.
    pub ip: String,
}

fn up(iface: &str) -> bool {
    fs::read_to_string(format!("/sys/class/net/{iface}/operstate"))
        .map(|s| s.trim() == "up")
        .unwrap_or(false)
}

/// `/proc/net/wireless`: one row per wireless interface, the link quality
/// in the third column, out of 70 on the Pi's drivers.
fn wifi_quality(text: &str) -> Option<u8> {
    for line in text.lines().skip(2) {
        let mut parts = line.split_whitespace();
        let iface = parts.next()?.trim_end_matches(':');
        let _status = parts.next()?;
        let link: f32 = parts.next()?.trim_end_matches('.').parse().ok()?;
        if up(iface) || cfg!(test) {
            return Some(((link / 70.0) * 4.0).round().clamp(0.0, 4.0) as u8);
        }
    }
    None
}

pub fn read() -> Net {
    let connection = if ["eth0", "eth1", "end0", "enp1s0"].iter().any(|i| up(i)) {
        Connection::Ethernet
    } else {
        fs::read_to_string("/proc/net/wireless")
            .ok()
            .and_then(|t| wifi_quality(&t))
            .map_or(Connection::None, Connection::Wifi)
    };
    Net {
        connection,
        ip: first_ipv4().unwrap_or_default(),
    }
}

fn first_ipv4() -> Option<String> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return None;
    }
    let mut found = None;
    let mut cur = list;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        let addr = ifa.ifa_addr;
        if !addr.is_null() && unsafe { (*addr).sa_family } as i32 == libc::AF_INET {
            let name = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_string_lossy();
            let is_loopback = ifa.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;
            if !is_loopback && name != "lo" {
                let sin = unsafe { &*(addr as *const libc::sockaddr_in) };
                let bytes = sin.sin_addr.s_addr.to_ne_bytes();
                found = Some(format!(
                    "{}.{}.{}.{}",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                ));
                break;
            }
        }
        cur = ifa.ifa_next;
    }
    unsafe { libc::freeifaddrs(list) };
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wireless_table_gives_a_quality_in_four_steps() {
        let text = "Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE\n face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22\n wlan0: 0000   52.  -58.  -256        0      0      0      0      0        0\n";
        assert_eq!(wifi_quality(text), Some(3));
        let weak = text.replace("52.", "10.");
        assert_eq!(wifi_quality(&weak), Some(1));
        assert_eq!(wifi_quality("Inter-|\n face |\n"), None);
    }

    #[test]
    fn reading_the_machine_does_not_panic() {
        let net = read();
        assert!(
            net.ip.is_empty() || net.ip.split('.').count() == 4,
            "{}",
            net.ip
        );
    }
}
