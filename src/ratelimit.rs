use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

/// Maximum number of live keys retained by one limiter.
const MAX_KEYS: usize = 10_000;
const MAX_KEY_BYTES: usize = 1024;

/// Format a rate-limit response without claiming that an unavailable retry
/// delay is zero seconds.
pub(crate) fn retry_after_message(prefix: &str, retry_after: u64) -> String {
    if retry_after == 0 {
        format!("{prefix} — try again later")
    } else {
        format!("{prefix} — try again in {retry_after} seconds")
    }
}

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
    state: Mutex<RateLimitState>,
    /// Maximum attempts allowed within the window.
    max_attempts: usize,
    /// Window duration.
    window: Duration,
}

/// Identifies one recorded attempt, so a refund can name the exact attempt it
/// is giving back.
///
/// Refunding used to pop the newest entry on the key, which is wrong the
/// moment two requests share one: a long-running successful login refunding
/// itself would erase a *failed* attempt recorded while it was still
/// verifying, handing the attacker a free guess. The id is the fix, and it is
/// why [`Reservation`] carries ids rather than key names alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptId(u64);

#[derive(Debug)]
struct Attempt {
    id: AttemptId,
    at: Instant,
}

#[derive(Debug)]
struct RateLimitState {
    attempts: HashMap<String, Vec<Attempt>>,
    last_sweep: Instant,
    /// Monotonic source of attempt ids.
    ///
    /// A `u64` at a million attempts a second takes ~584,000 years to wrap,
    /// and entries live for one window (minutes), so a live duplicate is not
    /// reachable. `checked_add` states that rather than assuming it: if the
    /// counter ever did saturate, allocation stops and reservations are
    /// refused, which fails closed.
    next_id: u64,
}

impl RateLimitState {
    fn allocate(&mut self) -> Option<AttemptId> {
        let id = AttemptId(self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        Some(id)
    }
}

impl RateLimiter {
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self {
            state: Mutex::new(RateLimitState {
                attempts: HashMap::new(),
                last_sweep: Instant::now(),
                next_id: 0,
            }),
            max_attempts,
            window,
        }
    }

    /// Remove all keys whose attempt lists are empty or fully expired.
    fn sweep(map: &mut HashMap<String, Vec<Attempt>>, now: Instant, window: Duration) {
        map.retain(|_, entries| {
            entries.retain(|entry| now.saturating_duration_since(entry.at) < window);
            !entries.is_empty()
        });
    }

    /// Keep the bounded table available to new identities. Expired entries are
    /// removed periodically; live entries are never evicted, so saturation
    /// cannot reset another identity's security state.
    fn make_room(state: &mut RateLimitState, now: Instant, window: Duration) -> bool {
        if now.saturating_duration_since(state.last_sweep) >= window {
            Self::sweep(&mut state.attempts, now, window);
            state.last_sweep = now;
        }
        state.attempts.len() < MAX_KEYS
    }

    /// Record an attempt for the given key.
    /// Returns `true` if the attempt is allowed, `false` if rate-limited.
    pub fn check(&self, key: &str) -> bool {
        self.reserve(key).is_some()
    }

    /// Record an attempt and hand back its id, so it can be refunded exactly.
    ///
    /// [`check`](Self::check) is this without the id, for callers that only
    /// want the yes or no. Anything that may later give the attempt back has
    /// to use this, because "the newest attempt on this key" stops meaning
    /// "my attempt" as soon as two requests share the key.
    pub fn reserve(&self, key: &str) -> Option<AttemptId> {
        if key.len() > MAX_KEY_BYTES {
            return None;
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());

        if !state.attempts.contains_key(key)
            && !Self::make_room(&mut state, now, self.window)
        {
            return None;
        }

        // Allocated before the entry is taken, so a saturated counter refuses
        // rather than recording an attempt it cannot name.
        let id = state.allocate()?;
        let entry = state.attempts.entry(key.to_string()).or_default();

        // Prune expired entries for this key
        entry.retain(|attempt| now.saturating_duration_since(attempt.at) < self.window);

        if entry.len() >= self.max_attempts {
            return None;
        }

        entry.push(Attempt { id, at: now });
        Some(id)
    }

    /// Give back one named attempt on `key`.
    ///
    /// The exact inverse of the [`reserve`](Self::reserve) that produced `id`,
    /// and the reason the login path can reserve its slots *before* doing
    /// expensive work rather than recording a failure after it.
    /// Reserve-then-refund is the only ordering that bounds concurrent work:
    /// peeking and recording later leaves a window in which any number of
    /// requests all see "under the limit" and proceed to burn CPU together.
    ///
    /// Naming the attempt matters as much as the ordering. Two requests can
    /// share a key, so "give back the newest" would let a slow successful
    /// request erase a failure recorded while it was still working. Only the
    /// entry with this id is removed; anything appended since stays.
    ///
    /// Takes the same lock and prunes on the same terms as every other method,
    /// so a refund cannot interleave with a reservation. Removing the last
    /// entry removes the key, keeping the bounded table free for other
    /// identities. Refunding an id that is absent or already expired is a
    /// no-op: it cannot manufacture budget.
    pub fn refund(&self, key: &str, id: AttemptId) {
        if key.len() > MAX_KEY_BYTES {
            return;
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = state.attempts.get_mut(key) else {
            return;
        };
        // Remove *this* attempt and nothing else. Attempts recorded after it,
        // by other requests on the same key, belong to those requests.
        entry.retain(|attempt| {
            attempt.id != id && now.saturating_duration_since(attempt.at) < self.window
        });
        if entry.is_empty() {
            state.attempts.remove(key);
        }
    }

    /// How many seconds until the oldest attempt in the window expires.
    pub fn retry_after(&self, key: &str) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match state.attempts.get(key) {
            Some(entries) if !entries.is_empty() => {
                let oldest = entries[0].at;
                let elapsed = now.saturating_duration_since(oldest);
                if elapsed < self.window {
                    (self.window - elapsed).as_secs() + 1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, key: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .attempts
            .contains_key(key)
    }
}

/// Two rate-limit slots held together, refunded together.
///
/// The login and re-authentication paths both limit on a pair of keys (the
/// address and the account) and both want the same thing: reserve before doing
/// expensive work, give the slots back if the work turns out to have been
/// legitimate. Doing that by hand invites the two mistakes this type removes:
/// forgetting to release the first key when the second is over its limit, and
/// refunding a key that was never reserved.
///
/// Deliberately NOT a `Drop` guard. The interesting outcome is the one where
/// nothing is refunded, and a guard that refunds on drop would make "spend the
/// attempt" the path you have to remember, which is backwards for a security
/// control. Refunding is explicit; failing is just returning.
pub struct Reservation<'a> {
    limiter: &'a RateLimiter,
    first: (String, AttemptId),
    second: (String, AttemptId),
}

/// Which half of a paired reservation refused admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationRejection {
    First,
    Second,
}

impl<'a> Reservation<'a> {
    /// Reserve both keys, or neither.
    ///
    /// Returns which key refused admission, having released the first if the
    /// second refused. Callers use that exact key for an honest Retry-After.
    pub fn acquire(
        limiter: &'a RateLimiter,
        first: &str,
        second: &str,
    ) -> Result<Self, ReservationRejection> {
        let first_id = limiter
            .reserve(first)
            .ok_or(ReservationRejection::First)?;
        let Some(second_id) = limiter.reserve(second) else {
            limiter.refund(first, first_id);
            return Err(ReservationRejection::Second);
        };
        Ok(Self {
            limiter,
            first: (first.to_string(), first_id),
            second: (second.to_string(), second_id),
        })
    }

    /// Give both slots back. Call this only when the attempt succeeded.
    pub fn refund(self) {
        self.limiter.refund(&self.first.0, self.first.1);
        self.limiter.refund(&self.second.0, self.second.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refund_gives_back_exactly_the_slot_its_reservation_took() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let first = rl.reserve("k").expect("free");
        assert!(rl.reserve("k").is_some());
        assert!(rl.reserve("k").is_none(), "the budget is spent");

        rl.refund("k", first);
        assert!(rl.reserve("k").is_some(), "the refunded slot came back");
        assert!(rl.reserve("k").is_none(), "and only that one slot");
    }

    /// The bug exact ids exist to prevent. Two requests share a key; the first
    /// is slow and succeeds, the second fails while it is still running. The
    /// success must give back *its own* attempt, not the newest one, which is
    /// the failure.
    #[test]
    fn a_refund_removes_only_its_own_attempt_not_the_newest() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        let slow_success = rl.reserve("shared").expect("free");
        let later_failure = rl.reserve("shared").expect("free");
        assert_ne!(slow_success, later_failure);

        rl.refund("shared", slow_success);

        assert!(rl.retry_after("shared") > 0, "the failure is still held");
        assert!(rl.reserve("shared").is_some());
        assert!(rl.reserve("shared").is_some());
        assert!(
            rl.reserve("shared").is_none(),
            "the later failure still costs its slot"
        );

        rl.refund("shared", later_failure);
        assert!(rl.reserve("shared").is_some());
    }

    #[test]
    fn refunding_the_last_attempt_frees_the_key() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let id = rl.reserve("k").expect("free");
        assert!(rl.contains_key("k"));

        rl.refund("k", id);
        assert!(
            !rl.contains_key("k"),
            "an emptied key must not hold a slot in the bounded table"
        );
    }

    #[test]
    fn refunding_an_unknown_key_or_a_spent_id_is_a_no_op() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        rl.refund("never-seen", AttemptId(999));
        assert!(!rl.contains_key("never-seen"));

        let id = rl.reserve("k").expect("free");
        rl.refund("k", id);
        // A second refund of the same id must not manufacture budget.
        rl.refund("k", id);
        assert!(rl.reserve("k").is_some());
        assert!(rl.reserve("k").is_none(), "still capped at one");
    }

    /// An id whose attempt has already aged out refunds nothing, because there
    /// is nothing left to refund.
    #[test]
    fn refunding_an_expired_attempt_is_a_no_op() {
        let rl = RateLimiter::new(1, Duration::from_secs(0));
        let id = rl.reserve("k").expect("free");
        rl.refund("k", id);
        assert!(rl.reserve("k").is_some());
    }

    #[test]
    fn an_oversized_key_is_refused_and_refunding_it_is_harmless() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        let huge = "x".repeat(MAX_KEY_BYTES + 1);
        assert!(rl.reserve(&huge).is_none());
        rl.refund(&huge, AttemptId(0));
        assert!(!rl.contains_key(&huge));
    }

    /// `retry_after` reflects what is still spent: a refunded attempt stops
    /// counting, a remaining failure keeps counting.
    #[test]
    fn retry_after_reflects_only_the_attempts_still_held() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let refunded = rl.reserve("k").expect("free");
        let kept = rl.reserve("k").expect("free");

        rl.refund("k", refunded);
        assert!(rl.retry_after("k") > 0, "the kept failure still counts");

        rl.refund("k", kept);
        assert_eq!(rl.retry_after("k"), 0, "nothing is held any more");
    }

    #[test]
    fn concurrent_checks_admit_at_most_max_attempts() {
        const MAX: usize = 5;
        const RACERS: usize = 64;
        let rl = std::sync::Arc::new(RateLimiter::new(MAX, Duration::from_secs(60)));
        let admitted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(RACERS));

        let mut handles = Vec::new();
        for _ in 0..RACERS {
            let (rl, admitted, barrier) = (rl.clone(), admitted.clone(), barrier.clone());
            handles.push(std::thread::spawn(move || {
                // Every thread arrives at the check at the same moment.
                barrier.wait();
                if rl.check("contended") {
                    admitted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            admitted.load(std::sync::atomic::Ordering::SeqCst),
            MAX,
            "exactly the budget, no more and no fewer"
        );
    }

    #[test]
    fn refunds_are_safe_under_contention() {
        let rl = std::sync::Arc::new(RateLimiter::new(4, Duration::from_secs(60)));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let (rl, barrier) = (rl.clone(), barrier.clone());
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                // Reserve and immediately give it back, the successful-login
                // shape. The budget must be intact afterwards.
                // Each thread refunds the id it was given, so the budget
                // must be intact afterwards however they interleave.
                if let Some(id) = rl.reserve("busy") {
                    rl.refund("busy", id);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        for _ in 0..4 {
            assert!(rl.check("busy"), "every slot survived the churn");
        }
        assert!(!rl.check("busy"));
    }

    #[test]
    fn retry_after_message_omits_zero_delay() {
        assert_eq!(
            retry_after_message("too many requests", 0),
            "too many requests — try again later"
        );
        assert_eq!(
            retry_after_message("too many requests", 3),
            "too many requests — try again in 3 seconds"
        );
    }

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
        let mut map: HashMap<String, Vec<Attempt>> = HashMap::new();
        let window = Duration::from_millis(1);

        // Insert an entry that will be expired by the time we sweep
        map.insert(
            "old".into(),
            vec![Attempt {
                id: AttemptId(0),
                at: Instant::now(),
            }],
        );

        // Wait for it to expire
        std::thread::sleep(Duration::from_millis(5));

        RateLimiter::sweep(&mut map, Instant::now(), window);
        assert!(map.is_empty(), "expired keys should be evicted");
    }

    #[test]
    fn full_table_rejects_new_identity_without_evicting_existing_state() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        for index in 0..MAX_KEYS {
            assert!(rl.check(&format!("identity-{index}")));
        }
        assert!(!rl.check("new-identity"));
        assert!(!rl.check("identity-0"));
    }

    #[test]
    fn expired_keys_are_swept_before_admitting_new_identity() {
        let rl = RateLimiter::new(1, Duration::ZERO);
        assert!(rl.check("expired"));
        assert!(rl.check("new"));
    }

    // ── LIF-422: reservation fails closed at saturation ──────
    //
    // A full table must refuse *new* identities rather than granting them
    // untracked attempts. `check()` is now the only admission path, so this is
    // stated directly against it (it used to also have to be stated against
    // `peek`, which answered `true` for absent keys while `record_failure`
    // refused to allocate one).
    #[test]
    fn a_full_table_refuses_new_identities_and_keeps_existing_ones() {
        let rl = RateLimiter::new(5, Duration::from_secs(3600));
        for index in 0..MAX_KEYS {
            assert!(rl.check(&format!("identity-{index}")));
        }

        assert!(
            !rl.check("new-identity"),
            "a saturated table must not admit an untracked identity"
        );
        assert!(!rl.contains_key("new-identity"));
        // An identity already in the table keeps its own budget.
        assert!(rl.check("identity-0"));
    }

    #[test]
    fn a_swept_table_admits_new_identities_again() {
        let rl = RateLimiter::new(5, Duration::from_secs(0));
        for index in 0..MAX_KEYS {
            assert!(rl.check(&format!("identity-{index}")));
        }
        // Every entry is expired (zero window), so the admission check sweeps
        // and finds room.
        assert!(rl.check("new-identity"));
    }

    #[test]
    fn oversized_keys_are_rejected_without_allocation() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        let oversized = "x".repeat(MAX_KEY_BYTES + 1);
        assert!(!rl.check(&oversized));
        rl.refund(&oversized, AttemptId(0));
        assert_eq!(rl.retry_after(&oversized), 0);
    }

    #[test]
    fn default_proxy_configuration_does_not_trust_loopback_headers() {
        let peer = "127.0.0.1".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.9".parse().unwrap());
        assert_eq!(client_ip(peer, &headers, &[]), "127.0.0.1");
    }

    // ── LIF-75: one failed attempt costs exactly one slot ────

    #[test]
    fn a_failed_attempt_costs_exactly_one_slot() {
        // The original bug was double counting: the login path called
        // `check()` (which records) *and* `record_failure()`, so five failures
        // spent ten slots. With reservations there is one recording call, and
        // a spent attempt is simply one that was never refunded.
        let rl = RateLimiter::new(5, Duration::from_secs(60));
        for i in 0..5 {
            assert!(rl.check("u"), "attempt {i} should be allowed");
        }
        assert!(!rl.check("u"), "6th attempt should be blocked");
    }

    // ── Paired reservations ──────────────────────────────────

    #[test]
    fn a_reservation_takes_both_keys_and_gives_both_back() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        let held = Reservation::acquire(&rl, "ip", "id").expect("both keys are free");
        assert!(!rl.check("ip"), "the address slot is held");
        assert!(!rl.check("id"), "the account slot is held");

        held.refund();
        assert!(rl.check("ip"));
        assert!(rl.check("id"));
    }

    /// The mistake this type removes: when the second key is over its limit,
    /// the first must be released. Otherwise one address at its limit quietly
    /// drains the budget of every account it touches.
    #[test]
    fn a_refused_second_key_releases_the_first() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("id"), "spend the account's only slot");

        assert!(matches!(
            Reservation::acquire(&rl, "ip", "id"),
            Err(ReservationRejection::Second)
        ));
        assert!(rl.check("ip"), "the address slot must have been given back");
    }

    #[test]
    fn a_refused_first_key_is_reported_exactly() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("ip"), "spend the address's only slot");

        assert!(matches!(
            Reservation::acquire(&rl, "ip", "id"),
            Err(ReservationRejection::First)
        ));
        assert!(rl.check("id"), "the untouched account key stays free");
    }

    #[test]
    fn a_reservation_that_is_never_refunded_stays_spent() {
        let rl = RateLimiter::new(2, Duration::from_secs(60));
        let _spent = Reservation::acquire(&rl, "ip", "id").expect("free");
        drop(_spent);
        // Dropping is not refunding: a failed attempt must cost something.
        assert!(rl.check("ip"));
        assert!(!rl.check("ip"));
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
