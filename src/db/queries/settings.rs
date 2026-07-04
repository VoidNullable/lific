//! Instance-wide settings store (LIF-210/211).
//!
//! A single-row table (`instance_settings`, id pinned to 1) holding the
//! admin-editable, runtime settings. `lific.toml`'s `auth.allow_signup` seeds
//! the row on first run; after that the DB row is authoritative and the
//! UI/CLI edit it live.

use rusqlite::{Connection, params};

use crate::error::LificError;

/// Hard caps so a settings write can't store something absurd.
const MAX_NAME_LEN: usize = 60;
const MAX_MESSAGE_LEN: usize = 280;
const MIN_SESSION_DAYS: i64 = 1;
const MAX_SESSION_DAYS: i64 = 365;

#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceSettings {
    pub allow_signup: bool,
    pub instance_name: Option<String>,
    pub signup_email_domains: Vec<String>,
    pub session_lifetime_days: i64,
    pub login_message: Option<String>,
    /// LIF-215: single-user mode. When true, the web UI may mint a session for
    /// the first admin without a password (see `/api/auth/auto-login`). Browser
    /// scope only — REST/MCP are unaffected. Dangerous on a public instance.
    pub web_auto_login: bool,
    /// LIF-196: runtime flip for project-scoped default-deny authorization
    /// (epic LIF-194). When true, `authz::require_role` enforces the full
    /// viewer/maintainer/lead membership matrix, including gated reads.
    /// See `src/authz.rs` and LIF-DOC-7.
    ///
    /// LIF-261: the *seed* default now depends on the install. `ensure` sets it
    /// on for a fresh DB (zero users at row-creation time) and off for an
    /// upgrade (users already exist); the code-level fallback in [`defaults`]
    /// stays off for a never-seeded read. Once the row exists it's
    /// authoritative — flip it at runtime via [`update`]. The agent-first flow
    /// keeps working with the default on because unbound API keys are
    /// operator-trusted.
    pub authz_enforced: bool,
}

/// Partial update. `None` = leave unchanged. For the nullable string fields
/// (`instance_name`, `login_message`) an empty/whitespace value clears to NULL.
#[derive(Default)]
pub struct InstanceSettingsPatch {
    pub allow_signup: Option<bool>,
    pub instance_name: Option<String>,
    pub signup_email_domains: Option<Vec<String>>,
    pub session_lifetime_days: Option<i64>,
    pub login_message: Option<String>,
    pub web_auto_login: Option<bool>,
    pub authz_enforced: Option<bool>,
}

/// Seed the settings row if it does not exist yet, using `allow_signup` as the
/// initial value (sourced from TOML at startup). No-op once the row exists.
///
/// LIF-261: on **row creation only**, `authz_enforced` is seeded to whether the
/// DB has zero users at that moment. A fresh install (no users yet) enforces
/// project-scoped authorization by default; an upgrade from a pre-2.0 instance
/// (users already exist) seeds it off, preserving today's behavior. Because
/// this is `INSERT OR IGNORE`, the row is authoritative once written — a later
/// `ensure` (e.g. every `lific start`) never re-evaluates or flips it, so an
/// admin who turns enforcement off stays off. The zero-user agent-first flow
/// still works with the default on because unbound API keys are
/// operator-trusted (see `src/authz.rs`).
pub fn ensure(conn: &Connection, allow_signup: bool) -> Result<(), LificError> {
    conn.execute(
        "INSERT OR IGNORE INTO instance_settings (id, allow_signup, authz_enforced)
         VALUES (1, ?1, (SELECT COUNT(*) FROM users) = 0)",
        params![allow_signup],
    )?;
    Ok(())
}

/// Code-level defaults, returned when the settings row has not been seeded yet.
/// Mirrors the column defaults in migration 023.
fn defaults() -> InstanceSettings {
    InstanceSettings {
        allow_signup: true,
        instance_name: None,
        signup_email_domains: Vec::new(),
        session_lifetime_days: 30,
        login_message: None,
        web_auto_login: false,
        authz_enforced: false,
    }
}

/// Read the current settings. Pure read: if the row has not been seeded yet
/// (no server start / `ensure` / `update` has run), returns code defaults so
/// this is safe on a read-only pool connection.
pub fn get(conn: &Connection) -> Result<InstanceSettings, LificError> {
    let found = conn
        .query_row(
            "SELECT allow_signup, instance_name, signup_email_domains,
                    session_lifetime_days, login_message, web_auto_login,
                    authz_enforced
             FROM instance_settings WHERE id = 1",
            [],
            |row| {
                let domains: String = row.get(2)?;
                Ok(InstanceSettings {
                    allow_signup: row.get::<_, i64>(0)? != 0,
                    instance_name: row.get(1)?,
                    signup_email_domains: parse_domains(&domains),
                    session_lifetime_days: row.get(3)?,
                    login_message: row.get(4)?,
                    web_auto_login: row.get::<_, i64>(5)? != 0,
                    authz_enforced: row.get::<_, i64>(6)? != 0,
                })
            },
        )
        .ok();
    Ok(found.unwrap_or_else(defaults))
}

/// Apply a partial update and return the new settings. Validates lengths,
/// session bounds, and email-domain shape; trims/normalizes as it goes.
pub fn update(
    conn: &Connection,
    patch: InstanceSettingsPatch,
) -> Result<InstanceSettings, LificError> {
    // Guarantee the single row exists before we UPDATE it (write conn).
    conn.execute("INSERT OR IGNORE INTO instance_settings (id) VALUES (1)", [])?;
    let cur = get(conn)?;

    let allow_signup = patch.allow_signup.unwrap_or(cur.allow_signup);

    let instance_name = match patch.instance_name {
        Some(s) => clean_optional(&s, MAX_NAME_LEN, "instance name")?,
        None => cur.instance_name,
    };

    let login_message = match patch.login_message {
        Some(s) => clean_optional(&s, MAX_MESSAGE_LEN, "login message")?,
        None => cur.login_message,
    };

    let domains = match patch.signup_email_domains {
        Some(list) => normalize_domains(list)?,
        None => cur.signup_email_domains,
    };

    let session_lifetime_days = match patch.session_lifetime_days {
        Some(d) => {
            if !(MIN_SESSION_DAYS..=MAX_SESSION_DAYS).contains(&d) {
                return Err(LificError::BadRequest(format!(
                    "session lifetime must be between {MIN_SESSION_DAYS} and {MAX_SESSION_DAYS} days"
                )));
            }
            d
        }
        None => cur.session_lifetime_days,
    };

    let web_auto_login = patch.web_auto_login.unwrap_or(cur.web_auto_login);
    let authz_enforced = patch.authz_enforced.unwrap_or(cur.authz_enforced);

    conn.execute(
        "UPDATE instance_settings
         SET allow_signup = ?1, instance_name = ?2, signup_email_domains = ?3,
             session_lifetime_days = ?4, login_message = ?5, web_auto_login = ?6,
             authz_enforced = ?7,
             updated_at = datetime('now')
         WHERE id = 1",
        params![
            allow_signup,
            instance_name,
            join_domains(&domains),
            session_lifetime_days,
            login_message,
            web_auto_login,
            authz_enforced,
        ],
    )?;

    get(conn)
}

/// Trim a free-text field; empty => None (clear); enforce a max length.
fn clean_optional(s: &str, max: usize, label: &str) -> Result<Option<String>, LificError> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > max {
        return Err(LificError::BadRequest(format!(
            "{label} must be {max} characters or fewer"
        )));
    }
    Ok(Some(t.to_string()))
}

/// Split the stored CSV into a clean domain list.
fn parse_domains(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn join_domains(domains: &[String]) -> String {
    domains.join(",")
}

/// Normalize + validate an incoming domain list: lowercase, strip a leading
/// '@', reject anything that isn't a plausible domain, and dedupe.
fn normalize_domains(list: Vec<String>) -> Result<Vec<String>, LificError> {
    let mut out: Vec<String> = Vec::new();
    for raw in list {
        let d = raw.trim().trim_start_matches('@').to_lowercase();
        if d.is_empty() {
            continue;
        }
        if !is_plausible_domain(&d) {
            return Err(LificError::BadRequest(format!(
                "'{raw}' is not a valid email domain"
            )));
        }
        if !out.contains(&d) {
            out.push(d);
        }
    }
    Ok(out)
}

/// Cheap domain sanity check (no RFC heroics): has a dot, only domain-legal
/// characters, no empty labels.
fn is_plausible_domain(d: &str) -> bool {
    if !d.contains('.') || d.starts_with('.') || d.ends_with('.') {
        return false;
    }
    if d.contains("..") {
        return false;
    }
    d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    #[test]
    fn defaults_are_sane() {
        let pool = conn();
        let c = pool.write().unwrap();
        let s = get(&c).unwrap();
        assert!(s.allow_signup);
        assert!(s.instance_name.is_none());
        assert!(s.signup_email_domains.is_empty());
        assert_eq!(s.session_lifetime_days, 30);
        assert!(s.login_message.is_none());
        assert!(!s.web_auto_login, "single-user auto-login is off by default");
        assert!(!s.authz_enforced, "authorization enforcement is off by default");
    }

    #[test]
    fn ensure_seeds_allow_signup_only_once() {
        let pool = conn();
        let c = pool.write().unwrap();
        ensure(&c, false).unwrap();
        assert!(!get(&c).unwrap().allow_signup, "first ensure seeds the value");
        // A later ensure with a different default must NOT overwrite.
        ensure(&c, true).unwrap();
        assert!(!get(&c).unwrap().allow_signup, "row already existed, left intact");
    }

    // ── LIF-261: authz_enforced seed default depends on the install ──────

    /// Insert a user directly so `ensure`'s `COUNT(*) FROM users` sees a
    /// non-fresh DB. Password hash content is irrelevant to the seed logic.
    fn seed_a_user(c: &Connection) {
        c.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES ('someone', 'someone@test.local', 'x', 'Someone', 0, 0)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn ensure_seeds_authz_enforced_on_for_a_fresh_install() {
        let pool = conn();
        let c = pool.write().unwrap();
        // Zero users at seed time → fresh install → enforce by default.
        ensure(&c, true).unwrap();
        assert!(
            get(&c).unwrap().authz_enforced,
            "a fresh DB (no users) must seed authz_enforced ON"
        );
    }

    #[test]
    fn ensure_seeds_authz_enforced_off_when_users_already_exist() {
        let pool = conn();
        let c = pool.write().unwrap();
        // An upgrade from a pre-2.0 instance: users predate the seed.
        seed_a_user(&c);
        ensure(&c, true).unwrap();
        assert!(
            !get(&c).unwrap().authz_enforced,
            "a DB that already has users must seed authz_enforced OFF (preserve behavior)"
        );
    }

    #[test]
    fn ensure_never_flips_authz_enforced_after_the_row_exists() {
        let pool = conn();
        let c = pool.write().unwrap();
        // First ensure on a fresh DB seeds ON and writes the row.
        ensure(&c, true).unwrap();
        assert!(get(&c).unwrap().authz_enforced);
        // Now users appear (people sign up). A later ensure — as every `lific
        // start` runs — must NOT re-evaluate and flip enforcement off.
        seed_a_user(&c);
        ensure(&c, true).unwrap();
        assert!(
            get(&c).unwrap().authz_enforced,
            "the seeded row is authoritative — a later ensure must never flip it"
        );
    }

    #[test]
    fn admin_can_turn_seeded_on_enforcement_back_off() {
        let pool = conn();
        let c = pool.write().unwrap();
        ensure(&c, true).unwrap();
        assert!(get(&c).unwrap().authz_enforced, "fresh install seeds ON");

        // An admin flips it off at runtime…
        let s = update(
            &c,
            InstanceSettingsPatch { authz_enforced: Some(false), ..Default::default() },
        )
        .unwrap();
        assert!(!s.authz_enforced);

        // …and a subsequent ensure (next `lific start`) leaves that choice intact.
        ensure(&c, true).unwrap();
        assert!(
            !get(&c).unwrap().authz_enforced,
            "ensure must respect the admin's later opt-out"
        );
    }

    #[test]
    fn update_sets_and_clears_name() {
        let pool = conn();
        let c = pool.write().unwrap();
        let s = update(
            &c,
            InstanceSettingsPatch { instance_name: Some("  Acme Eng  ".into()), ..Default::default() },
        )
        .unwrap();
        assert_eq!(s.instance_name.as_deref(), Some("Acme Eng")); // trimmed
        // Empty clears back to NULL.
        let s = update(
            &c,
            InstanceSettingsPatch { instance_name: Some("   ".into()), ..Default::default() },
        )
        .unwrap();
        assert!(s.instance_name.is_none());
    }

    #[test]
    fn update_normalizes_domains() {
        let pool = conn();
        let c = pool.write().unwrap();
        let s = update(
            &c,
            InstanceSettingsPatch {
                signup_email_domains: Some(vec![
                    "@Acme.com".into(),
                    "acme.com".into(), // dup after normalize
                    " sub.Example.CO ".into(),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(s.signup_email_domains, vec!["acme.com", "sub.example.co"]);
    }

    #[test]
    fn update_rejects_bad_domain_and_bad_session() {
        let pool = conn();
        let c = pool.write().unwrap();
        assert!(update(
            &c,
            InstanceSettingsPatch { signup_email_domains: Some(vec!["not a domain".into()]), ..Default::default() },
        )
        .is_err());
        assert!(update(
            &c,
            InstanceSettingsPatch { session_lifetime_days: Some(0), ..Default::default() },
        )
        .is_err());
        assert!(update(
            &c,
            InstanceSettingsPatch { session_lifetime_days: Some(999), ..Default::default() },
        )
        .is_err());
    }

    #[test]
    fn update_toggles_signup_and_session() {
        let pool = conn();
        let c = pool.write().unwrap();
        let s = update(
            &c,
            InstanceSettingsPatch {
                allow_signup: Some(false),
                session_lifetime_days: Some(14),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!s.allow_signup);
        assert_eq!(s.session_lifetime_days, 14);
    }

    // LIF-215: the single-user auto-login flag round-trips and defaults off.
    #[test]
    fn update_toggles_web_auto_login() {
        let pool = conn();
        let c = pool.write().unwrap();
        assert!(!get(&c).unwrap().web_auto_login);

        let s = update(
            &c,
            InstanceSettingsPatch { web_auto_login: Some(true), ..Default::default() },
        )
        .unwrap();
        assert!(s.web_auto_login);
        // Persisted, and an unrelated patch leaves it intact.
        let s = update(
            &c,
            InstanceSettingsPatch { instance_name: Some("Solo".into()), ..Default::default() },
        )
        .unwrap();
        assert!(s.web_auto_login, "unrelated patch must not clear the flag");

        let s = update(
            &c,
            InstanceSettingsPatch { web_auto_login: Some(false), ..Default::default() },
        )
        .unwrap();
        assert!(!s.web_auto_login);
    }

    // LIF-196: the authz_enforced runtime flag round-trips and defaults off.
    #[test]
    fn update_toggles_authz_enforced() {
        let pool = conn();
        let c = pool.write().unwrap();
        assert!(!get(&c).unwrap().authz_enforced);

        let s = update(
            &c,
            InstanceSettingsPatch { authz_enforced: Some(true), ..Default::default() },
        )
        .unwrap();
        assert!(s.authz_enforced);
        // Persisted, and an unrelated patch leaves it intact.
        let s = update(
            &c,
            InstanceSettingsPatch { instance_name: Some("Solo".into()), ..Default::default() },
        )
        .unwrap();
        assert!(s.authz_enforced, "unrelated patch must not clear the flag");

        let s = update(
            &c,
            InstanceSettingsPatch { authz_enforced: Some(false), ..Default::default() },
        )
        .unwrap();
        assert!(!s.authz_enforced);
    }
}
