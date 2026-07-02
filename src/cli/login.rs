//! `lific login` / `lific logout` — device-flow authentication (LIF-252).
//!
//! The device flow (RFC 8628) works everywhere a browser-redirect OAuth flow
//! does not: over SSH, in containers, in CI, and for agents. The CLI asks the
//! server for a `device_code` + short `user_code`, shows the human a URL and
//! the code, and polls the token endpoint until the human approves on any
//! device.
//!
//! Two entry shapes, mirroring Stripe's non-interactive pattern:
//!
//! - **Interactive** (`lific login` at a TTY): request a device code, print the
//!   URL + code, poll to completion, store the token.
//! - **Non-interactive** (`--non-interactive`, or stdin not a TTY): request a
//!   device code, print it as JSON with a `next_step`, and exit 0 without
//!   polling. A follow-up `lific login --complete <device_code>` polls to
//!   completion once a human has approved.
//!
//! ## Testability
//!
//! Network calls go through the [`DeviceFlow`] trait so the polling loop and
//! its interval/backoff arithmetic are unit-testable against a scripted fake
//! (see tests). [`poll_backoff`] is the pure interval calculation.

use std::time::Duration;

use serde::Deserialize;

use crate::config::Config;

/// The device-code grant type string (RFC 8628).
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Resolve the base URL: explicit `--url` wins, else `server.public_url`, else
/// `http://127.0.0.1:<port>`.
pub fn resolve_base_url(url: Option<&str>, cfg: &Config) -> String {
    if let Some(u) = url {
        return u.trim().trim_end_matches('/').to_string();
    }
    if let Some(pu) = cfg.server.public_url.as_deref() {
        return pu.trim().trim_end_matches('/').to_string();
    }
    format!("http://127.0.0.1:{}", cfg.server.port)
}

/// The response from `POST /oauth/device_authorization`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuthResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

/// Terminal outcome of a token poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    /// The token was minted; carries the raw access token.
    Approved(String),
    /// The user denied the request.
    Denied,
    /// The device code expired before approval.
    Expired,
}

/// A single non-terminal poll signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollSignal {
    Pending,
    SlowDown,
    Terminal(PollOutcome),
}

/// Abstraction over the two network operations the login flow needs, so the
/// polling loop is testable without a live server.
pub trait DeviceFlow {
    /// `POST {base}/oauth/device_authorization`.
    fn request_device_code(&self, label: Option<&str>) -> Result<DeviceAuthResponse, String>;
    /// One `POST {base}/oauth/token` with the device grant. Maps the RFC 8628
    /// error/status into a [`PollSignal`].
    fn poll_token(&self, device_code: &str) -> Result<PollSignal, String>;
    /// Best-effort `POST {base}/oauth/revoke` (used by logout). Returns Ok even
    /// if the server rejects it; only transport errors surface.
    fn revoke(&self, token: &str) -> Result<(), String>;
}

/// Compute the next sleep interval given the base interval and how many
/// consecutive `slow_down` signals we've seen. RFC 8628 says to increase the
/// interval by 5 seconds on each `slow_down`.
pub fn poll_backoff(base_interval: u64, slow_downs: u32) -> u64 {
    base_interval + 5 * (slow_downs as u64)
}

/// Run the polling loop to a terminal outcome, sleeping `sleep` between polls.
/// `deadline_secs` is a hard cap (the device code's `expires_in`); if exceeded
/// we return `Expired`. Factored to take a `sleep` closure so tests can run it
/// instantly.
pub fn poll_loop<F, S>(
    flow: &F,
    device_code: &str,
    interval: u64,
    deadline_secs: u64,
    mut sleep: S,
) -> Result<PollOutcome, String>
where
    F: DeviceFlow,
    S: FnMut(Duration),
{
    let start = std::time::Instant::now();
    let mut slow_downs: u32 = 0;
    loop {
        if start.elapsed().as_secs() >= deadline_secs {
            return Ok(PollOutcome::Expired);
        }
        match flow.poll_token(device_code)? {
            PollSignal::Terminal(outcome) => return Ok(outcome),
            PollSignal::Pending => {
                sleep(Duration::from_secs(poll_backoff(interval, slow_downs)));
            }
            PollSignal::SlowDown => {
                slow_downs += 1;
                sleep(Duration::from_secs(poll_backoff(interval, slow_downs)));
            }
        }
    }
}

/// Build the non-interactive JSON payload printed by `--non-interactive`.
pub fn non_interactive_json(resp: &DeviceAuthResponse, base: &str) -> serde_json::Value {
    serde_json::json!({
        "verification_uri": resp.verification_uri,
        "verification_uri_complete": resp.verification_uri_complete,
        "user_code": resp.user_code,
        "device_code": resp.device_code,
        "interval": resp.interval,
        "expires_in": resp.expires_in,
        "next_step": format!("lific login --complete {} --url {}", resp.device_code, base),
    })
}

// ── reqwest-backed implementation ────────────────────────────────────────

/// Live [`DeviceFlow`] backed by a blocking reqwest client against `base`.
pub struct HttpDeviceFlow {
    base: String,
    client: reqwest::blocking::Client,
}

impl HttpDeviceFlow {
    pub fn new(base: &str) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            client,
        })
    }
}

impl DeviceFlow for HttpDeviceFlow {
    fn request_device_code(&self, label: Option<&str>) -> Result<DeviceAuthResponse, String> {
        let url = format!("{}/oauth/device_authorization", self.base);
        let mut form: Vec<(&str, &str)> = Vec::new();
        if let Some(l) = label {
            form.push(("client_name", l));
        }
        let resp = self
            .client
            .post(&url)
            .form(&form)
            .send()
            .map_err(|e| format!("device authorization request failed: {e}"))?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            return Err(format!("device authorization failed (HTTP {code}): {body}"));
        }
        resp.json::<DeviceAuthResponse>()
            .map_err(|e| format!("invalid device authorization response: {e}"))
    }

    fn poll_token(&self, device_code: &str) -> Result<PollSignal, String> {
        let url = format!("{}/oauth/token", self.base);
        let form = [
            ("grant_type", DEVICE_CODE_GRANT),
            ("device_code", device_code),
        ];
        let resp = self
            .client
            .post(&url)
            .form(&form)
            .send()
            .map_err(|e| format!("token poll failed: {e}"))?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().unwrap_or(serde_json::json!({}));
        if status.is_success() {
            let token = body
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "token response missing access_token".to_string())?;
            return Ok(PollSignal::Terminal(PollOutcome::Approved(token.to_string())));
        }
        let err = body.get("error").and_then(|v| v.as_str()).unwrap_or("");
        Ok(classify_poll_error(err))
    }

    fn revoke(&self, token: &str) -> Result<(), String> {
        let url = format!("{}/oauth/revoke", self.base);
        let form = [("token", token)];
        // Revoke requires auth; present the token itself as the bearer.
        let _ = self
            .client
            .post(&url)
            .bearer_auth(token)
            .form(&form)
            .send()
            .map_err(|e| format!("revoke request failed: {e}"))?;
        Ok(())
    }
}

/// Map an RFC 8628 §3.5 `error` code to a poll signal.
pub fn classify_poll_error(error: &str) -> PollSignal {
    match error {
        "authorization_pending" => PollSignal::Pending,
        "slow_down" => PollSignal::SlowDown,
        "access_denied" => PollSignal::Terminal(PollOutcome::Denied),
        "expired_token" => PollSignal::Terminal(PollOutcome::Expired),
        // Any other error (invalid_grant, etc.) is terminal-ish; treat as
        // expired so the loop stops rather than spinning forever.
        _ => PollSignal::Terminal(PollOutcome::Expired),
    }
}

// ── Command entry points ─────────────────────────────────────────────────

/// Arguments for [`run_login`], mirrored from the CLI enum.
pub struct LoginArgs {
    pub url: Option<String>,
    pub non_interactive: bool,
    pub complete: Option<String>,
    pub label: Option<String>,
    pub no_store: bool,
}

/// `lific login`. Returns `Ok(())` on success (or after printing the
/// non-interactive JSON), `Err` on failure.
pub fn run_login(args: &LoginArgs, cfg: &Config, json: bool) -> Result<(), String> {
    let base = resolve_base_url(args.url.as_deref(), cfg);
    let flow = HttpDeviceFlow::new(&base)?;
    run_login_with_flow(args, &base, &flow, json, crate::cli::term::stdin_is_tty())
}

/// Testable core: takes an injected [`DeviceFlow`] and TTY state.
pub fn run_login_with_flow<F: DeviceFlow>(
    args: &LoginArgs,
    base: &str,
    flow: &F,
    json: bool,
    stdin_tty: bool,
) -> Result<(), String> {
    // `--complete <device_code>`: skip requesting a new code; poll the given one.
    if let Some(device_code) = &args.complete {
        // Interval unknown here (we didn't request the code), so use the RFC
        // default of 5s and a generous 15-minute deadline.
        let outcome = poll_loop(flow, device_code, 5, 900, std::thread::sleep)?;
        return finish(args, base, outcome, json);
    }

    let resp = flow.request_device_code(args.label.as_deref())?;

    // Non-interactive (explicit flag OR no TTY): print JSON and exit without
    // polling — a second `--complete` call finishes the login.
    if args.non_interactive || !stdin_tty {
        let payload = non_interactive_json(&resp, base);
        println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
        return Ok(());
    }

    // Interactive: show the code prominently, then poll.
    let complete = resp
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| resp.verification_uri.clone());
    eprintln!();
    eprintln!("  To sign in, visit:");
    eprintln!("    {}", resp.verification_uri);
    eprintln!("  and enter the code:");
    eprintln!();
    eprintln!("    {}", resp.user_code);
    eprintln!();
    eprintln!("  Or open this URL directly:");
    eprintln!("    {complete}");
    eprintln!();
    eprintln!("  Waiting for approval…");

    let outcome = poll_loop(
        flow,
        &resp.device_code,
        resp.interval.max(1),
        resp.expires_in,
        std::thread::sleep,
    )?;
    finish(args, base, outcome, json)
}

/// Handle a terminal poll outcome: store the token (unless `--no-store`) and
/// print a confirmation, or report denial/expiry as an error.
fn finish(args: &LoginArgs, base: &str, outcome: PollOutcome, json: bool) -> Result<(), String> {
    match outcome {
        PollOutcome::Approved(token) => {
            if args.no_store {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "approved",
                            "stored": false,
                            "access_token": token,
                        }))
                        .unwrap_or_default()
                    );
                } else {
                    println!("Approved. Token (not stored):");
                    println!("{token}");
                }
                return Ok(());
            }
            crate::cli::credentials::store(base, &token)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "approved",
                        "stored": true,
                        "url": base,
                    }))
                    .unwrap_or_default()
                );
            } else {
                println!("Signed in to {base}. Token stored.");
            }
            Ok(())
        }
        PollOutcome::Denied => Err("login denied by user".to_string()),
        PollOutcome::Expired => {
            Err("login timed out / device code expired — run `lific login` again".to_string())
        }
    }
}

/// `lific logout`: delete the stored credential and best-effort revoke it.
pub fn run_logout(url: Option<&str>, cfg: &Config, json: bool) -> Result<(), String> {
    let base = resolve_base_url(url, cfg);
    // Grab the token first so we can revoke it before deleting.
    let existing = crate::cli::credentials::load(&base);
    if let Some(token) = &existing
        && let Ok(flow) = HttpDeviceFlow::new(&base)
    {
        // Best-effort; ignore revoke failures (server may be down).
        let _ = flow.revoke(token);
    }
    let removed = crate::cli::credentials::delete(&base);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "url": base,
                "removed": removed,
            }))
            .unwrap_or_default()
        );
    } else if removed {
        println!("Signed out of {base}.");
    } else {
        println!("No stored credential for {base}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── pure arithmetic ──────────────────────────────────────

    #[test]
    fn backoff_adds_5s_per_slow_down() {
        assert_eq!(poll_backoff(5, 0), 5);
        assert_eq!(poll_backoff(5, 1), 10);
        assert_eq!(poll_backoff(5, 3), 20);
        assert_eq!(poll_backoff(2, 2), 12);
    }

    #[test]
    fn classify_maps_rfc8628_errors() {
        assert_eq!(classify_poll_error("authorization_pending"), PollSignal::Pending);
        assert_eq!(classify_poll_error("slow_down"), PollSignal::SlowDown);
        assert_eq!(
            classify_poll_error("access_denied"),
            PollSignal::Terminal(PollOutcome::Denied)
        );
        assert_eq!(
            classify_poll_error("expired_token"),
            PollSignal::Terminal(PollOutcome::Expired)
        );
        // Unknown → treated as terminal (expired) so the loop stops.
        assert_eq!(
            classify_poll_error("invalid_grant"),
            PollSignal::Terminal(PollOutcome::Expired)
        );
    }

    #[test]
    fn resolve_base_url_precedence() {
        let mut cfg = Config::default();
        cfg.server.port = 3999;
        // Explicit --url wins, trailing slash trimmed.
        assert_eq!(
            resolve_base_url(Some("http://h:1/"), &cfg),
            "http://h:1"
        );
        // Else public_url.
        cfg.server.public_url = Some("https://lific.example/".into());
        assert_eq!(resolve_base_url(None, &cfg), "https://lific.example");
        // Else loopback:port.
        cfg.server.public_url = None;
        assert_eq!(resolve_base_url(None, &cfg), "http://127.0.0.1:3999");
    }

    #[test]
    fn non_interactive_json_shape() {
        let resp = DeviceAuthResponse {
            device_code: "DEV123".into(),
            user_code: "BCDF-GHJK".into(),
            verification_uri: "http://h/oauth/device".into(),
            verification_uri_complete: Some("http://h/oauth/device?user_code=BCDF-GHJK".into()),
            expires_in: 900,
            interval: 5,
        };
        let v = non_interactive_json(&resp, "http://h");
        assert_eq!(v["user_code"], "BCDF-GHJK");
        assert_eq!(v["device_code"], "DEV123");
        assert_eq!(v["interval"], 5);
        assert_eq!(v["expires_in"], 900);
        assert_eq!(
            v["verification_uri_complete"],
            "http://h/oauth/device?user_code=BCDF-GHJK"
        );
        assert_eq!(
            v["next_step"],
            "lific login --complete DEV123 --url http://h"
        );
    }

    // ── scripted fake for the polling loop ───────────────────

    /// A fake that returns a scripted sequence of poll signals, one per call.
    struct FakeFlow {
        signals: RefCell<Vec<PollSignal>>,
        polls: RefCell<u32>,
        device: DeviceAuthResponse,
    }

    impl FakeFlow {
        fn new(signals: Vec<PollSignal>) -> Self {
            Self {
                signals: RefCell::new(signals),
                polls: RefCell::new(0),
                device: DeviceAuthResponse {
                    device_code: "DEV".into(),
                    user_code: "BCDF-GHJK".into(),
                    verification_uri: "http://h/oauth/device".into(),
                    verification_uri_complete: Some("http://h/oauth/device?user_code=BCDF-GHJK".into()),
                    expires_in: 900,
                    interval: 5,
                },
            }
        }
    }

    impl DeviceFlow for FakeFlow {
        fn request_device_code(&self, _label: Option<&str>) -> Result<DeviceAuthResponse, String> {
            Ok(self.device.clone())
        }
        fn poll_token(&self, _device_code: &str) -> Result<PollSignal, String> {
            *self.polls.borrow_mut() += 1;
            let mut sigs = self.signals.borrow_mut();
            if sigs.is_empty() {
                Ok(PollSignal::Terminal(PollOutcome::Expired))
            } else {
                Ok(sigs.remove(0))
            }
        }
        fn revoke(&self, _token: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn poll_loop_pending_then_approved() {
        let flow = FakeFlow::new(vec![
            PollSignal::Pending,
            PollSignal::Pending,
            PollSignal::Terminal(PollOutcome::Approved("lific_at_xyz".into())),
        ]);
        let sleeps = RefCell::new(0u32);
        let outcome = poll_loop(&flow, "DEV", 5, 900, |_| {
            *sleeps.borrow_mut() += 1;
        })
        .unwrap();
        assert_eq!(outcome, PollOutcome::Approved("lific_at_xyz".into()));
        // Two pending signals → two sleeps before the terminal poll.
        assert_eq!(*sleeps.borrow(), 2);
        assert_eq!(*flow.polls.borrow(), 3);
    }

    #[test]
    fn poll_loop_slow_down_increases_backoff() {
        let flow = FakeFlow::new(vec![
            PollSignal::SlowDown,
            PollSignal::SlowDown,
            PollSignal::Terminal(PollOutcome::Approved("t".into())),
        ]);
        let durations = RefCell::new(Vec::<u64>::new());
        let outcome = poll_loop(&flow, "DEV", 5, 900, |d| {
            durations.borrow_mut().push(d.as_secs());
        })
        .unwrap();
        assert_eq!(outcome, PollOutcome::Approved("t".into()));
        // First slow_down → +5 (10s), second → +10 (15s).
        assert_eq!(*durations.borrow(), vec![10, 15]);
    }

    #[test]
    fn poll_loop_denied_is_terminal() {
        let flow = FakeFlow::new(vec![
            PollSignal::Pending,
            PollSignal::Terminal(PollOutcome::Denied),
        ]);
        let outcome = poll_loop(&flow, "DEV", 5, 900, |_| {}).unwrap();
        assert_eq!(outcome, PollOutcome::Denied);
    }

    #[test]
    fn poll_loop_honors_deadline() {
        // All-pending script with a zero-second deadline → immediate Expired.
        let flow = FakeFlow::new(vec![PollSignal::Pending; 3]);
        let outcome = poll_loop(&flow, "DEV", 5, 0, |_| {}).unwrap();
        assert_eq!(outcome, PollOutcome::Expired);
    }

    #[test]
    fn non_interactive_prints_json_and_exits_without_polling() {
        let flow = FakeFlow::new(vec![]); // must NOT be polled
        let args = LoginArgs {
            url: None,
            non_interactive: true,
            complete: None,
            label: None,
            no_store: true,
        };
        // stdin_tty=true but --non-interactive set → still non-interactive.
        run_login_with_flow(&args, "http://h", &flow, true, true).unwrap();
        assert_eq!(*flow.polls.borrow(), 0, "must not poll in non-interactive mode");
    }

    #[test]
    fn no_tty_forces_non_interactive() {
        let flow = FakeFlow::new(vec![]);
        let args = LoginArgs {
            url: None,
            non_interactive: false,
            complete: None,
            label: None,
            no_store: true,
        };
        // stdin_tty=false → non-interactive even without the flag.
        run_login_with_flow(&args, "http://h", &flow, true, false).unwrap();
        assert_eq!(*flow.polls.borrow(), 0);
    }
}
