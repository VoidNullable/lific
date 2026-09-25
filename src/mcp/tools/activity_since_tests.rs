//! LIF-482: `get_activity(since=...)` reads forward from a cursor.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn activity(m: &LificMcp, identifier: &str, since: Option<&str>) -> String {
    m.get_activity(Parameters(GetActivityInput {
        identifier: identifier.into(),
        since: since.map(Into::into),
        ..Default::default()
    }))
}

#[test]
fn since_returns_only_later_entries_oldest_first() {
    let (m, _guard) = mcp();
    seed_project(&m, "Cursor", "CUR");
    seed_issue(&m, "CUR", "Earlier work");
    // One-second timestamps: move the first batch back instead of sleeping.
    m.write(|conn| Ok(conn.execute("UPDATE audit_log SET ts = datetime('now', '-2 hours')", [])?))
        .unwrap();
    let cursor: String = m
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 hour')",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    seed_issue(&m, "CUR", "Later first");
    seed_issue(&m, "CUR", "Later second");

    let out = activity(&m, "CUR", Some(&cursor));
    assert!(
        out.starts_with("2 activity entries for CUR after "),
        "got: {out}"
    );
    assert!(out.contains("oldest first"), "got: {out}");
    assert!(!out.contains("Earlier work"), "got: {out}");
    let first = out.find("Later first").expect("first later entry");
    let second = out.find("Later second").expect("second later entry");
    assert!(first < second, "oldest first: {out}");

    // Without a cursor the whole feed comes back newest-first.
    let all = activity(&m, "CUR", None);
    assert!(all.contains("Earlier work"), "got: {all}");
    assert!(
        all.find("Later second").unwrap() < all.find("Later first").unwrap(),
        "newest first: {all}"
    );
}

#[test]
fn since_past_the_last_entry_says_nothing_happened() {
    let (m, _guard) = mcp();
    seed_project(&m, "Cursor", "CUR");
    let out = activity(&m, "CUR", Some("2999-01-01"));
    assert_eq!(out, "No activity for CUR after 2999-01-01 00:00:00 UTC.");
}

#[test]
fn an_unparseable_since_is_an_error() {
    let (m, _guard) = mcp();
    seed_project(&m, "Cursor", "CUR");
    let out = activity(&m, "CUR", Some("last tuesday"));
    assert!(out.starts_with("Error: "), "got: {out}");
    assert!(out.contains("invalid since"), "got: {out}");
}
