//! The television's own IPv4 addresses (`getifaddrs`), which nothing else in the application
//! reads: the home-network scan for a streaming server starts from them.

use std::net::Ipv4Addr;

/// The IPv4 addresses of the interfaces that are up, loopback and `0.0.0.0` left out, each once,
/// in the order the system lists them. Empty when the system lists none (or the call fails).
pub fn local_ipv4() -> Vec<Ipv4Addr> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-pointer; on success the list is read below and freed once.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = list;
    while !at.is_null() {
        // SAFETY: `at` is a node of the list `getifaddrs` returned, alive until `freeifaddrs`.
        let entry = unsafe { &*at };
        at = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: `ifa_addr` is non-null and points at a sockaddr whose family says its shape.
        let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
        if family != libc::AF_INET {
            continue;
        }
        // SAFETY: an `AF_INET` sockaddr is a `sockaddr_in`.
        let raw = unsafe { (*entry.ifa_addr.cast::<libc::sockaddr_in>()).sin_addr.s_addr };
        let addr = Ipv4Addr::from(u32::from_be(raw));
        let up = entry.ifa_flags & libc::IFF_UP as u32 != 0;
        if usable(up, addr) && !out.contains(&addr) {
            out.push(addr);
        }
    }
    // SAFETY: `list` came from a successful `getifaddrs` and is freed exactly once.
    unsafe { libc::freeifaddrs(list) };
    out
}

/// Is an interface's address one a scan may start from: up, not loopback, not unspecified.
fn usable(up: bool, addr: Ipv4Addr) -> bool {
    up && !addr.is_loopback() && !addr.is_unspecified()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_up_non_loopback_address_is_usable() {
        assert!(usable(true, Ipv4Addr::new(192, 168, 1, 20)));
        assert!(usable(true, Ipv4Addr::new(10, 0, 0, 5)));
        assert!(!usable(false, Ipv4Addr::new(192, 168, 1, 20)), "an interface that is down");
        assert!(!usable(true, Ipv4Addr::LOCALHOST));
        assert!(!usable(true, Ipv4Addr::new(127, 0, 1, 1)), "all of 127/8 is loopback");
        assert!(!usable(true, Ipv4Addr::UNSPECIFIED));
    }

    /// Whatever this host has, the list holds no loopback or repeat.
    #[test]
    fn the_host_list_has_no_loopback_and_no_repeats() {
        let list = local_ipv4();
        assert!(list.iter().all(|a| !a.is_loopback() && !a.is_unspecified()), "{list:?}");
        let mut sorted = list.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len());
    }
}
