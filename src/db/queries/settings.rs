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
}

/// Seed the settings row if it does not exist yet, using `allow_signup` as the
/// initial value (sourced from TOML at startup). No-op once the row exists.
pub fn ensure(conn: &Connection, allow_signup: bool) -> Result<(), LificError> {
    conn.execute(
        "INSERT OR IGNORE INTO instance_settings (id, allow_signup) VALUES (1, ?1)",
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
    }
}

/// Read the current settings. Pure read: if the row has not been seeded yet
/// (no server start / `ensure` / `update` has run), returns code defaults so
/// this is safe on a read-only pool connection.
pub fn get(conn: &Connection) -> Result<InstanceSettings, LificError> {
    let found = conn
        .query_row(
            "SELECT allow_signup, instance_name, signup_email_domains,
                    session_lifetime_days, login_message
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

    conn.execute(
        "UPDATE instance_settings
         SET allow_signup = ?1, instance_name = ?2, signup_email_domains = ?3,
             session_lifetime_days = ?4, login_message = ?5, updated_at = datetime('now')
         WHERE id = 1",
        params![
            allow_signup,
            instance_name,
            join_domains(&domains),
            session_lifetime_days,
            login_message,
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
}
