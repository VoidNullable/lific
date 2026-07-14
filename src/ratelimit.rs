use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

/// Maximum number of keys before a forced full sweep is triggered.
const MAX_KEYS: usize = 10_000;

/// A validated IP address or CIDR range trusted to supply client-IP headers.
///
/// Plain IPs represent a single host (`/32` for IPv4 or `/128` for IPv6).
/// Config is parsed once at startup; callers only use [`contains`] per request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpNetwork {
    address: IpAddr,
    prefix_len: u8,
}

impl IpNetwork {
    /// Parse an IPv4/IPv6 address or CIDR range.
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let (address_text, prefix_text) = input
            .split_once('/')
            .map_or((input, None), |(address, prefix)| (address, Some(prefix)));
        let address = address_text
            .parse::<IpAddr>()
            .map_err(|_| "must be an IP address or CIDR range".to_string())?;
        let max_prefix = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix_len = match prefix_text {
            Some(prefix) => prefix
                .parse::<u8>()
                .map_err(|_| format!("invalid prefix length {prefix:?}"))?,
            None => max_prefix,
        };
        if prefix_len > max_prefix {
            return Err(format!(
                "prefix length {prefix_len} exceeds the {max_prefix}-bit address family"
            ));
        }
        Ok(Self {
            address,
            prefix_len,
        })
    }

    /// Whether this range contains `ip`. IPv4-mapped IPv6 peers are normalized
    /// to IPv4 first, so `::ffff:10.0.0.1` matches `10.0.0.0/8`.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.address, normalize_ip(ip)) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                prefix_matches(&network.octets(), &ip.octets(), self.prefix_len)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                prefix_matches(&network.octets(), &ip.octets(), self.prefix_len)
            }
            _ => false,
        }
    }
}

/// Parse all configured trusted proxy ranges, preserving a useful index/value
/// in errors so invalid configuration fails loudly instead of being skipped.
pub fn parse_trusted_proxies(entries: &[String]) -> Result<Vec<IpNetwork>, String> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            IpNetwork::parse(entry)
                .map_err(|error| format!("trusted_proxies[{index}] ({entry:?}): {error}"))
        })
        .collect()
}

/// Return `ip` in its canonical rate-limit/audit-key form. IPv4-mapped IPv6
/// addresses have the fixed `::ffff:0:0/96` prefix and must share IPv4 buckets.
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(ipv6) = ip else {
        return ip;
    };
    let octets = ipv6.octets();
    if octets[..10] == [0; 10] && octets[10] == 0xff && octets[11] == 0xff {
        IpAddr::V4(std::net::Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ))
    } else {
        IpAddr::V6(ipv6)
    }
}

fn prefix_matches(network: &[u8], ip: &[u8], prefix_len: u8) -> bool {
    let whole_bytes = usize::from(prefix_len / 8);
    let remaining_bits = prefix_len % 8;
    network[..whole_bytes] == ip[..whole_bytes]
        && (remaining_bits == 0
            || (network[whole_bytes] & (!0u8 << (8 - remaining_bits)))
                == (ip[whole_bytes] & (!0u8 << (8 - remaining_bits))))
}

fn header_ip(headers: &HeaderMap, name: &str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
}

/// Return the first untrusted address while walking every XFF header entry from
/// right to left. `None` means the chain was malformed or entirely trusted.
fn x_forwarded_for_client_ip(headers: &HeaderMap, trusted_proxies: &[IpNetwork]) -> Option<IpAddr> {
    let mut entries = Vec::new();
    for value in headers.get_all("x-forwarded-for").iter() {
        let line = value.to_str().ok()?;
        entries.extend(line.split(','));
    }

    for entry in entries.into_iter().rev() {
        let ip = normalize_ip(entry.trim().parse::<IpAddr>().ok()?);
        if !trusted_proxies.iter().any(|range| range.contains(ip)) {
            return Some(ip);
        }
    }
    None
}

/// Derive a rate-limit/audit client-IP key from the TCP peer and, only when the
/// peer is a configured trusted proxy, validated proxy headers.
///
/// An untrusted direct peer always wins, preventing an attacker from rotating
/// client-controlled `X-Forwarded-For` values into fresh limiter buckets. For
/// a trusted peer, all XFF header lines form one ordered chain: walk it right
/// to left, skip trusted proxy hops, and use the first untrusted IP. A malformed
/// or all-trusted XFF chain falls back to the peer; `X-Real-IP` is consulted
/// only when XFF is absent. Header-derived values are always strict `IpAddr`s.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted_proxies: &[IpNetwork]) -> String {
    let peer = normalize_ip(peer);
    if !trusted_proxies.iter().any(|range| range.contains(peer)) {
        return peer.to_string();
    }

    let client = if headers.contains_key("x-forwarded-for") {
        x_forwarded_for_client_ip(headers, trusted_proxies).unwrap_or(peer)
    } else {
        header_ip(headers, "x-real-ip")
            .map(normalize_ip)
            .unwrap_or(peer)
    };
    normalize_ip(client).to_string()
}

/// Simple in-memory rate limiter.
/// Tracks attempts per key (e.g. username or IP) within a sliding window.
/// Expired keys are evicted periodically to prevent unbounded memory growth.
#[derive(Debug)]
pub struct RateLimiter {
    /// (key -> list of attempt timestamps)
    attempts: Mutex<HashMap<String, Vec<Instant>>>,
    /// Maximum attempts allowed within the window.
    max_attempts: usize,
    /// Window duration.
    window: Duration,
}

impl RateLimiter {
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window,
        }
    }

    /// Remove all keys whose attempt lists are empty or fully expired.
    fn sweep(map: &mut HashMap<String, Vec<Instant>>, window: Duration) {
        let now = Instant::now();
        map.retain(|_, entries| {
            entries.retain(|t| now.duration_since(*t) < window);
            !entries.is_empty()
        });
    }

    /// Record an attempt for the given key.
    /// Returns `true` if the attempt is allowed, `false` if rate-limited.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());

        // Evict expired keys if map is getting large
        if map.len() > MAX_KEYS {
            Self::sweep(&mut map, self.window);
        }

        let entry = map.entry(key.to_string()).or_default();

        // Prune expired entries for this key
        entry.retain(|t| now.duration_since(*t) < self.window);

        if entry.len() >= self.max_attempts {
            return false;
        }

        entry.push(now);
        true
    }

    /// Test whether an attempt *would* be allowed, WITHOUT recording it.
    ///
    /// This exists so the login path can separate the "are we over the
    /// limit?" question from the "record a failure" action. Previously
    /// login called `check()` (which records on pass) AND
    /// `record_failure()`, so each failed login consumed two slots and
    /// halved the effective limit (LIF-75). Login now `peek()`s, then
    /// records exactly one failure via `record_failure()` on auth failure.
    pub fn peek(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        match map.get_mut(key) {
            Some(entry) => {
                entry.retain(|t| now.duration_since(*t) < self.window);
                entry.len() < self.max_attempts
            }
            None => true,
        }
    }

    /// Record a failed attempt without checking first (for tracking after auth failure).
    pub fn record_failure(&self, key: &str) {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());

        if map.len() > MAX_KEYS {
            Self::sweep(&mut map, self.window);
        }

        let entry = map.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        entry.push(now);
    }

    /// How many seconds until the oldest attempt in the window expires.
    pub fn retry_after(&self, key: &str) -> u64 {
        let now = Instant::now();
        let map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(key) {
            Some(entries) if !entries.is_empty() => {
                let oldest = entries[0];
                let elapsed = now.duration_since(oldest);
                if elapsed < self.window {
                    (self.window - elapsed).as_secs() + 1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_under_limit() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
    }

    #[test]
    fn blocks_over_limit() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user1"));
        assert!(!rl.check("user1")); // blocked
    }

    #[test]
    fn different_keys_independent() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("user1"));
        assert!(rl.check("user2")); // different key, still allowed
        assert!(!rl.check("user1")); // same key, blocked
    }

    #[test]
    fn retry_after_nonzero_when_limited() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        rl.check("user1");
        assert!(rl.retry_after("user1") > 0);
    }

    #[test]
    fn sweep_removes_expired_keys() {
        let mut map: HashMap<String, Vec<Instant>> = HashMap::new();
        let window = Duration::from_millis(1);

        // Insert an entry that will be expired by the time we sweep
        map.insert("old".into(), vec![Instant::now()]);

        // Wait for it to expire
        std::thread::sleep(Duration::from_millis(5));

        RateLimiter::sweep(&mut map, window);
        assert!(map.is_empty(), "expired keys should be evicted");
    }

    // ── LIF-75: peek() is non-recording ──────────────────────

    #[test]
    fn peek_does_not_record() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        // Peeking repeatedly never consumes the budget.
        assert!(rl.peek("user1"));
        assert!(rl.peek("user1"));
        assert!(rl.peek("user1"));
        // A single recorded failure is enough to hit the limit of 1.
        rl.record_failure("user1");
        assert!(
            !rl.peek("user1"),
            "should be at limit after exactly one recorded failure"
        );
    }

    #[test]
    fn peek_reflects_recorded_failures_one_to_one() {
        // Proves the double-counting fix: N recorded failures == N slots,
        // not 2N. With max 5, the 5th failure is still allowed, 6th blocked.
        let rl = RateLimiter::new(5, Duration::from_secs(60));
        for i in 0..5 {
            assert!(rl.peek("u"), "attempt {i} should be allowed");
            rl.record_failure("u");
        }
        assert!(!rl.peek("u"), "6th attempt should be blocked");
    }

    // ── LIF-206: trusted proxy client-IP extraction ──────────

    #[test]
    fn untrusted_peer_ignores_spoofed_forwarded_headers() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into()]).unwrap();
        let peer = "203.0.113.5".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
        h.insert("x-real-ip", "198.51.100.4".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "203.0.113.5");
    }

    #[test]
    fn repeated_forwarded_for_lines_use_the_proxy_appended_line() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into()]).unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.append("x-forwarded-for", "198.51.100.10".parse().unwrap());
        h.append("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "203.0.113.9");
    }

    #[test]
    fn trusted_proxy_chain_skips_trusted_intermediate_hops() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into(), "10.0.0.0/8".into()])
            .unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.9, 10.0.0.2".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "203.0.113.9");
    }

    #[test]
    fn all_trusted_forwarded_for_hops_fall_back_to_peer() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into(), "10.0.0.0/8".into()])
            .unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "10.0.0.2, 127.0.0.2".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "127.0.0.1");
    }

    #[test]
    fn trusted_peer_falls_back_to_x_real_ip() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into()]).unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "198.51.100.4".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "198.51.100.4");
    }

    #[test]
    fn malformed_forwarded_for_does_not_fall_through_to_x_real_ip() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into()]).unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4:5678".parse().unwrap());
        h.insert("x-real-ip", "198.51.100.4".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &trusted), "127.0.0.1");
    }

    #[test]
    fn trusted_peer_without_valid_headers_falls_back_to_peer() {
        let trusted = parse_trusted_proxies(&["127.0.0.0/8".into()]).unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        let h = HeaderMap::new();
        assert_eq!(client_ip(peer, &h, &trusted), "127.0.0.1");
    }

    #[test]
    fn cidr_matcher_handles_v4_v6_and_ipv4_mapped_ipv6() {
        let v4_exact = IpNetwork::parse("192.0.2.1").unwrap();
        assert!(v4_exact.contains("192.0.2.1".parse().unwrap()));
        assert!(!v4_exact.contains("192.0.2.2".parse().unwrap()));

        let v4_everything = IpNetwork::parse("0.0.0.0/0").unwrap();
        assert!(v4_everything.contains("0.0.0.0".parse().unwrap()));
        assert!(v4_everything.contains("255.255.255.255".parse().unwrap()));

        let v4_range = IpNetwork::parse("10.0.0.0/8").unwrap();
        assert!(v4_range.contains("10.255.255.255".parse().unwrap()));
        assert!(!v4_range.contains("11.0.0.0".parse().unwrap()));
        assert!(v4_range.contains("::ffff:10.1.2.3".parse().unwrap()));

        let v6_loopback = IpNetwork::parse("::1/128").unwrap();
        assert!(v6_loopback.contains("::1".parse().unwrap()));
        assert!(!v6_loopback.contains("::2".parse().unwrap()));

        let v6_everything = IpNetwork::parse("::/0").unwrap();
        assert!(v6_everything.contains("::1".parse().unwrap()));
        assert!(v6_everything.contains("2001:db8::1".parse().unwrap()));

        let v6_range = IpNetwork::parse("2001:db8:1234:5678::/61").unwrap();
        assert!(v6_range.contains("2001:db8:1234:567f::1".parse().unwrap()));
        assert!(!v6_range.contains("2001:db8:1234:5680::1".parse().unwrap()));
    }

    #[test]
    fn ipv4_mapped_ipv6_is_normalized_for_bucket_keys() {
        let peer = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(client_ip(peer, &HeaderMap::new(), &[]), "192.0.2.1");
    }

    #[test]
    fn invalid_trusted_proxy_range_is_rejected() {
        let error = parse_trusted_proxies(&["10.0.0.0/99".into()]).unwrap_err();
        assert!(error.contains("trusted_proxies[0]"));
    }
}
