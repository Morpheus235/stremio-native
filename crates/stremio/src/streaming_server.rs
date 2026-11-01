//! The Stremio streaming server: the address the viewer gives, how to tell a live server from
//! anything else on the network, and the URL a torrent plays from.
//!
//! A server (stremio-service on a computer, a NAS, a Docker image) answers `GET {base}settings`
//! with `{"values": {...}}` and plays a torrent from `{base}{infoHash}/{fileIdx}` with the
//! stream's trackers as `tr=` parameters (stremio-core's `streaming_url`). Everything here is
//! pure: the application runs the requests, and scans the home network with [`scan_targets`].

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::Deserialize;

/// The port a streaming server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 11470;

/// The longest version string kept: the server's own, shown to the viewer.
const VERSION_MAX: usize = 24;

/// What a live server says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// `serverVersion` of its settings, when it sends one.
    pub version: Option<String>,
}

/// `input` as a server base URL (`http://192.168.1.20:11470/`), or `None` when it is not one.
///
/// A typed address may leave out the scheme (`http://` is assumed) and the port (`:11470` is
/// added to a plain `http` address; an `https` address keeps its own, 443). Only http and https
/// are servers, a host is needed, a user name or password in the address is refused, a query or
/// fragment is dropped, and the result always ends in `/` so a path base keeps its path when
/// joined.
pub fn normalize_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (scheme, rest) = match input.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("http".to_string(), input),
    };
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (host, port) = split_authority(authority)?;
    let path = tail.split(['?', '#']).next().unwrap_or("");
    let mut out = format!("{scheme}://{}", host.to_ascii_lowercase());
    match (port, scheme.as_str()) {
        (Some(port), _) => out.push_str(&format!(":{port}")),
        (None, "http") => out.push_str(&format!(":{DEFAULT_PORT}")),
        (None, _) => {}
    }
    out.push_str(if path.is_empty() { "/" } else { path });
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

/// An authority's host and optional port, both checked.
fn split_authority(authority: &str) -> Option<(&str, Option<u16>)> {
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        // an IPv6 literal: `[::1]` or `[::1]:11470`
        let close = authority.find(']')?;
        authority[1..close].parse::<std::net::Ipv6Addr>().ok()?;
        let after = &authority[close + 1..];
        (&authority[..=close], if after.is_empty() { None } else { Some(after.strip_prefix(':')?) })
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        let name = host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
        if host.is_empty() || !name || host.starts_with('.') || host.ends_with('.') {
            return None;
        }
        // digits and dots only must be a whole dotted quad: the television's resolver reads
        // `192.168.1` as 192.168.0.1 and a bare number as an address too
        if host.chars().all(|c| c.is_ascii_digit() || c == '.') && host.parse::<Ipv4Addr>().is_err() {
            return None;
        }
        (host, port)
    };
    // `host:` with nothing after the colon is a typo, not "no port"
    let port = match port {
        None => None,
        Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p != 0)?),
    };
    Some((host, port))
}

/// What the viewer reads for `base`: the address without `http://` and the closing `/` of a bare
/// host (`192.168.1.20:11470`); anything else as it is.
pub fn display(base: &str) -> String {
    let (scheme, rest) = if let Some(rest) = base.strip_prefix("http://") { ("", rest) } else { base.split_at(base.find("://").map_or(0, |i| i + 3)) };
    let rest = rest.strip_suffix('/').filter(|r| !r.contains('/')).unwrap_or(rest);
    format!("{scheme}{rest}")
}

/// Where a server answers whether it is alive. `base` ends in `/` ([`normalize_url`]).
pub fn settings_url(base: &str) -> String {
    join(base, "settings")
}

#[derive(Deserialize)]
struct Settings {
    values: Option<serde_json::Value>,
}

/// A server is alive when its `settings` body is a JSON object holding an object `values`;
/// anything else (a router's page, another service's JSON) is not.
pub fn parse_settings(body: &[u8]) -> Option<ServerInfo> {
    let settings: Settings = serde_json::from_slice(body).ok()?;
    let values = settings.values?;
    let values = values.as_object()?;
    let version = values
        .get("serverVersion")
        .and_then(serde_json::Value::as_str)
        .map(|v| v.trim().chars().filter(|c| !c.is_control()).take(VERSION_MAX).collect::<String>())
        .filter(|v| !v.is_empty());
    Some(ServerInfo { version })
}

/// `raw` as the 40-character lower-case hex info hash a server's path takes, or `None` when it is
/// not one: an addon sends it, and it becomes a path segment of the viewer's own server, so
/// anything else (`removeall?x=`, a `/`, `..`, a wrong length) must never reach a URL. Accepts 40
/// hex digits in either case, and the 32-letter base32 form (RFC 4648, `A-Z2-7`, either case,
/// no padding) some addons send, converted to hex.
pub fn normalize_info_hash(raw: &str) -> Option<String> {
    let raw = raw.trim();
    match raw.len() {
        40 if raw.bytes().all(|b| b.is_ascii_hexdigit()) => Some(raw.to_ascii_lowercase()),
        32 => {
            let (mut bits, mut have) = (0u32, 0u32);
            let mut hex = String::with_capacity(40);
            for b in raw.bytes() {
                let v = match b.to_ascii_uppercase() {
                    c @ b'A'..=b'Z' => c - b'A',
                    c @ b'2'..=b'7' => c - b'2' + 26,
                    _ => return None,
                };
                bits = (bits << 5) | u32::from(v);
                have += 5;
                while have >= 8 {
                    have -= 8;
                    hex.push_str(&format!("{:02x}", (bits >> have) & 0xff));
                }
                bits &= (1 << have) - 1;
            }
            Some(hex)
        }
        _ => None,
    }
}

/// The URL a torrent plays from: `{base}{infoHash}/{fileIdx}` and one `tr=` parameter per source,
/// as given (`tracker:udp://…`, `dht:…`). `file_idx` `None` is `-1`: the server picks the
/// largest file. No earlier request is needed.
pub fn torrent_url(base: &str, info_hash: &str, file_idx: Option<u32>, sources: &[String]) -> String {
    let index = file_idx.map_or_else(|| "-1".to_string(), |i| i.to_string());
    let mut url = join(base, &format!("{}/{index}", info_hash.trim().to_lowercase()));
    for (i, source) in sources.iter().enumerate() {
        url.push(if i == 0 { '?' } else { '&' });
        url.push_str("tr=");
        url.push_str(&form_encode(source));
    }
    url
}

/// `base` + `path`, exactly one `/` between them: a base with a path keeps it.
fn join(base: &str, path: &str) -> String {
    format!("{}/{path}", base.trim_end_matches('/'))
}

/// A query value as `application/x-www-form-urlencoded` writes it (the encoding stremio-core's
/// URL builder uses): letters, digits and `*-._` as they are, a space as `+`, the rest `%XX`.
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b if b.is_ascii_alphanumeric() || b"*-._".contains(&b) => out.push(b as char),
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Is `base` (from [`normalize_url`]) a plain `http://` address whose host is an IPv4 literal that
/// is private (10/8, 172.16/12, 192.168/16), loopback or link-local, or an IPv6 loopback,
/// unique-local (fc00::/7) or link-local (fe80::/10) literal? Only those are probed with the LAN
/// deadlines (1.5 s): a name, an https address or a public IP gets the normal ones.
pub fn is_lan_base(base: &str) -> bool {
    let Some(rest) = base.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    if let Some(inner) = authority.strip_prefix('[') {
        let Some((host, _)) = inner.split_once(']') else { return false };
        let Ok(ip) = host.parse::<Ipv6Addr>() else { return false };
        let first = ip.segments()[0];
        return ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80;
    }
    let host = authority.rsplit_once(':').map_or(authority, |(host, _)| host);
    host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local())
}

/// The other hosts of `own`'s /24 (`.1` to `.254`), in address order: where a scan looks for a
/// server on the home network.
pub fn scan_targets(own: Ipv4Addr) -> Vec<Ipv4Addr> {
    let [a, b, c, mine] = own.octets();
    (1..=254u8).filter(|host| *host != mine).map(|host| Ipv4Addr::new(a, b, c, host)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_becomes_a_base_url() {
        let n = |s: &str| normalize_url(s);
        assert_eq!(n("192.168.1.20").as_deref(), Some("http://192.168.1.20:11470/"));
        assert_eq!(n("  192.168.1.20  ").as_deref(), Some("http://192.168.1.20:11470/"), "trimmed");
        assert_eq!(n("192.168.1.20:8080").as_deref(), Some("http://192.168.1.20:8080/"));
        assert_eq!(n("http://host:8080").as_deref(), Some("http://host:8080/"));
        assert_eq!(n("http://host").as_deref(), Some("http://host:11470/"), "a plain http address gets the default port");
        assert_eq!(n("HTTP://Host.Local:11470/").as_deref(), Some("http://host.local:11470/"), "scheme and host are lower-cased");
        assert_eq!(n("https://a.b/c/").as_deref(), Some("https://a.b/c/"), "https keeps its own port and path");
        assert_eq!(n("https://a.b").as_deref(), Some("https://a.b/"));
        assert_eq!(n("https://a.b:8443/x").as_deref(), Some("https://a.b:8443/x/"), "the closing slash is added");
        assert_eq!(n("http://nas.local/stremio?x=1#top").as_deref(), Some("http://nas.local:11470/stremio/"), "query and fragment are dropped");
        assert_eq!(n("[::1]:11470").as_deref(), Some("http://[::1]:11470/"));
        assert_eq!(n("http://[fe80::1]").as_deref(), Some("http://[fe80::1]:11470/"));
    }

    #[test]
    fn what_is_not_a_server_address_is_refused() {
        for bad in [
            "", "   ", "ftp://host", "file:///etc/passwd", "ws://host", "://host", "http://", "http:///path", "http://:8080",
            "host:", "host:0", "host:65536", "host:abc", "ho st", "http://user:pw@host", "mailto:a@b.c", "http://.host", "http://host.",
            "http://[::1", "http://[nope]:1", "http://[::1]x", "garbage!", "a/b c",
            // a host of only digits and dots must be a whole dotted quad: the TV's resolver reads
            // `192.168.1` as 192.168.0.1 and `11470` as a number
            "192.168.1", "192.168.1:11470", "http://192.168", "11470", "http://10", "1.2.3.4.5", "192.168.1.256", "192.168.1.", "1..2",
            "http://192.168.001.5", "http://999.1.1.1",
        ] {
            assert_eq!(normalize_url(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_whole_dotted_quad_and_names_with_digits_are_still_servers() {
        let n = |s: &str| normalize_url(s);
        assert_eq!(n("192.168.1.20").as_deref(), Some("http://192.168.1.20:11470/"));
        assert_eq!(n("0.0.0.0:80").as_deref(), Some("http://0.0.0.0:80/"));
        assert_eq!(n("nas1.local").as_deref(), Some("http://nas1.local:11470/"));
        assert_eq!(n("3com").as_deref(), Some("http://3com:11470/"), "a name that merely starts with digits");
        assert_eq!(n("192.168.1.x").as_deref(), Some("http://192.168.1.x:11470/"));
    }

    #[test]
    fn a_normalized_address_stays_the_same() {
        for s in ["192.168.1.20", "http://nas.local/stremio", "https://a.b/c/", "[::1]:9"] {
            let once = normalize_url(s).unwrap();
            assert_eq!(normalize_url(&once).as_deref(), Some(once.as_str()), "{s}");
        }
    }

    #[test]
    fn the_viewer_reads_the_short_address() {
        assert_eq!(display("http://192.168.1.20:11470/"), "192.168.1.20:11470");
        assert_eq!(display("https://a.b/"), "https://a.b");
        assert_eq!(display("http://nas.local:11470/stremio/"), "nas.local:11470/stremio/");
    }

    #[test]
    fn the_settings_url_keeps_a_path_base() {
        assert_eq!(settings_url("http://192.168.1.20:11470/"), "http://192.168.1.20:11470/settings");
        assert_eq!(settings_url("https://a.b/c/"), "https://a.b/c/settings");
    }

    #[test]
    fn a_server_is_alive_only_with_an_object_of_values() {
        let info = |b: &str| parse_settings(b.as_bytes());
        assert_eq!(info(r#"{"options":[],"values":{"serverVersion":"4.20.8","cacheSize":2147483648},"baseUrl":"http://x"}"#), Some(ServerInfo { version: Some("4.20.8".into()) }));
        assert_eq!(info(r#"{"values":{}}"#), Some(ServerInfo { version: None }), "no version is still alive");
        assert_eq!(info(r#"{"values":{"serverVersion":""}}"#), Some(ServerInfo { version: None }));
        assert_eq!(info(r#"{"values":{"serverVersion":7}}"#), Some(ServerInfo { version: None }), "a version that is not text is none");
        for dead in ["", "<html>router</html>", "[]", "null", r#"{}"#, r#"{"values":null}"#, r#"{"values":[]}"#, r#"{"values":"x"}"#, r#"{"settings":{}}"#] {
            assert_eq!(info(dead), None, "{dead:?}");
        }
        let long = format!(r#"{{"values":{{"serverVersion":"{}"}}}}"#, "9".repeat(100));
        assert_eq!(info(&long).and_then(|i| i.version).map(|v| v.len()), Some(VERSION_MAX), "an overlong version is cut");
    }

    #[test]
    fn an_info_hash_is_forty_hex_or_thirty_two_base32() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(normalize_info_hash(hex).as_deref(), Some(hex));
        assert_eq!(normalize_info_hash(&hex.to_uppercase()).as_deref(), Some(hex));
        // the same 20 bytes in RFC 4648 base32
        assert_eq!(normalize_info_hash("AERUKZ4JVPG66AJDIVTYTK6N54ASGRLH").as_deref(), Some(hex));
        assert_eq!(normalize_info_hash("aerukz4jvpg66ajdivtytk6n54asgrlh").as_deref(), Some(hex));
        assert_eq!(normalize_info_hash(&"A".repeat(32)).as_deref(), Some(&"0".repeat(40)[..]));
        assert_eq!(normalize_info_hash(&"7".repeat(32)).as_deref(), Some(&"f".repeat(40)[..]));
        for bad in ["", "abc", "removeall?x=", "../x", &hex[..39], &format!("{hex}0"), &"z".repeat(40), &"1".repeat(32), &"8".repeat(32), &"A".repeat(31)] {
            assert_eq!(normalize_info_hash(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_a_home_network_literal_is_a_lan_server() {
        for lan in [
            "http://192.168.1.20:11470/", "http://10.0.0.5:11470/", "http://10.255.255.255:1/", "http://172.16.0.1:11470/", "http://172.31.9.9:80/",
            "http://127.0.0.1:11470/", "http://169.254.1.1:11470/", "http://[::1]:11470/", "http://[fd12:3456::1]:11470/", "http://[fc00::1]:1/",
            "http://[fe80::1]:11470/", "http://[febf::1]:1/", "http://192.168.1.20:11470/stremio/",
        ] {
            assert!(is_lan_base(lan), "{lan}");
        }
        for far in [
            "https://192.168.1.20/", "https://192.168.1.20:11470/", "http://nas.local:11470/", "http://8.8.8.8:11470/", "http://172.15.0.1:1/",
            "http://172.32.0.1:1/", "http://192.169.1.1:1/", "http://11.0.0.1:1/", "http://[2001:db8::1]:11470/", "http://[fec0::1]:1/",
            "http://[::ffff:8.8.8.8]:1/", "http://192.168.1.20.example.com:1/", "ftp://192.168.1.20/", "192.168.1.20", "", "http://",
            "http://192.168.1.20@evil.example:1/", "http://user@192.168.1.20:1/",
        ] {
            assert!(!is_lan_base(far), "{far}");
        }
    }

    #[test]
    fn a_torrent_plays_from_its_hash_and_file() {
        let base = "http://192.168.1.20:11470/";
        assert_eq!(torrent_url(base, "ABCDEF0123", Some(2), &[]), "http://192.168.1.20:11470/abcdef0123/2");
        assert_eq!(torrent_url(base, "abcdef0123", None, &[]), "http://192.168.1.20:11470/abcdef0123/-1", "no file: the server picks the largest");
        assert_eq!(torrent_url(base, "abc", Some(0), &[]), "http://192.168.1.20:11470/abc/0", "file 0 is a file");
    }

    #[test]
    fn a_torrent_passes_each_source_as_an_encoded_tr() {
        let base = "http://192.168.1.20:11470/";
        let sources = vec!["tracker:udp://tracker.example:80/announce".to_string(), "dht:ABC".to_string(), "tracker:http://a.b/c d?x=1&y=2".to_string()];
        assert_eq!(
            torrent_url(base, "abc", Some(1), &sources),
            "http://192.168.1.20:11470/abc/1?tr=tracker%3Audp%3A%2F%2Ftracker.example%3A80%2Fannounce&tr=dht%3AABC&tr=tracker%3Ahttp%3A%2F%2Fa.b%2Fc+d%3Fx%3D1%26y%3D2"
        );
    }

    #[test]
    fn a_torrent_url_keeps_the_path_of_its_base() {
        assert_eq!(torrent_url("https://a.b/stremio/", "abc", None, &["dht:x".into()]), "https://a.b/stremio/abc/-1?tr=dht%3Ax");
        // a base missing its closing slash still joins with exactly one
        assert_eq!(torrent_url("http://h:1", "abc", Some(3), &[]), "http://h:1/abc/3");
    }

    #[test]
    fn a_scan_covers_the_other_254_hosts_of_the_subnet() {
        let targets = scan_targets(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(targets.len(), 253);
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 1, 20)), "not the TV itself");
        assert_eq!((targets[0], targets[252]), (Ipv4Addr::new(192, 168, 1, 1), Ipv4Addr::new(192, 168, 1, 254)));
        assert!(targets.iter().all(|a| a.octets()[..3] == [192, 168, 1]));
        assert!(!targets.contains(&Ipv4Addr::new(192, 168, 1, 0)) && !targets.contains(&Ipv4Addr::new(192, 168, 1, 255)));
        assert_eq!(scan_targets(Ipv4Addr::new(10, 0, 0, 0)).len(), 254, "an own address outside .1-.254 removes nobody");
        assert_eq!(scan_targets(Ipv4Addr::new(10, 0, 0, 254)).len(), 253);
    }
}
