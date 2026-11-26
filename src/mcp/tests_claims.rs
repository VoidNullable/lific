//! LIF-525: claims through update_issue, and how list_issues, get_issue and
//! the briefing read them.

use rmcp::handler::server::wrapper::Parameters;

use super::*;

fn as_user<R>(user: &models::AuthUser, f: impl FnOnce() -> R) -> R {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_user(
            Some(user.clone()),
            || async { f() },
        ))
}

struct Setup {
    db: crate::db::DbPool,
    maintainer: models::AuthUser,
    lead: models::AuthUser,
    _guard: McpTestGuard,
}

/// The shared membership fixture: project MEM, enforcement on.
fn setup() -> Setup {
    let (db, _admin, lead, maintainer, _viewer, _outsider, _project_id) =
        crate::api::test_helpers::setup_membership_test();
    let au = |u: models::User| models::AuthUser {
        id: u.id,
        username: u.username,
        display_name: u.display_name,
        is_admin: u.is_admin,
    };
    Setup {
        db,
        maintainer: au(maintainer),
        lead: au(lead),
        _guard: acquire_test_guard(),
    }
}

/// One MCP session: a stdio server process of its own on the shared
/// database, so it mints its own worker id.
fn session(s: &Setup) -> LificMcp {
    LificMcp::new(s.db.clone())
}

fn create(s: &Setup, m: &LificMcp, title: &str, status: &str) -> String {
    let out = as_user(&s.maintainer, || {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: title.into(),
            status: Some(status.into()),
            ..Default::default()
        }))
    });
    assert!(out.starts_with("Created"), "{out}");
    let id = out
        .split_whitespace()
        .find(|word| word.starts_with("MEM-"))
        .unwrap();
    id.trim_end_matches(|c: char| !c.is_ascii_digit())
        .to_string()
}

fn update(s: &Setup, m: &LificMcp, input: UpdateIssueInput) -> String {
    as_user(&s.maintainer, || m.update_issue(Parameters(input)))
}

fn claim(s: &Setup, m: &LificMcp, identifier: &str) -> String {
    update(
        s,
        m,
        UpdateIssueInput {
            identifier: identifier.into(),
            claim: Some(true),
            ..Default::default()
        },
    )
}

fn workable(s: &Setup, m: &LificMcp) -> String {
    as_user(&s.maintainer, || {
        m.list_issues(Parameters(ListIssuesInput {
            project: Some("MEM".into()),
            workable: Some(true),
            ..Default::default()
        }))
        .text
    })
}

fn get(s: &Setup, m: &LificMcp, identifier: &str) -> String {
    as_user(&s.maintainer, || {
        m.get_issue(Parameters(GetIssueInput {
            identifier: identifier.into(),
            include_comments: Some("none".into()),
            ..Default::default()
        }))
    })
}

fn stored(s: &Setup, identifier: &str) -> (String, String, String, i64) {
    let conn = s.db.read().unwrap();
    let id = queries::resolve_identifier(&conn, identifier).unwrap();
    conn.query_row(
        "SELECT c.claimed_at, c.expires_at, i.status, i.seq
           FROM issue_claims c JOIN issues i ON i.id = c.issue_id WHERE c.issue_id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .unwrap()
}

#[test]
fn two_sessions_on_one_key_cannot_hold_the_same_issue() {
    let s = setup();
    let (first, second) = (session(&s), session(&s));
    let issue = create(&s, &first, "Contested", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");

    let held = claim(&s, &first, &issue);
    let worker = first.session_worker().unwrap();
    assert!(
        held.contains(&format!(
            "Claim held until 2026-11-26 12:15 UTC as worker {worker}"
        )),
        "{held}"
    );
    assert!(held.contains("| active |"), "todo becomes active: {held}");

    let refused = claim(&s, &second, &issue);
    assert!(refused.starts_with("Error:"), "{refused}");
    assert!(
        refused.contains(&format!(
            "{issue} is claimed by @maintainer (worker {worker}) until 2026-11-26 12:15 UTC"
        )),
        "{refused}"
    );

    // The first session sees it as workable, the second does not.
    assert!(workable(&s, &first).contains("Contested"));
    assert_eq!(workable(&s, &second), "No issues found.");

    // Released, the second session takes it.
    let released = update(
        &s,
        &first,
        UpdateIssueInput {
            identifier: issue.clone(),
            claim: Some(false),
            ..Default::default()
        },
    );
    assert!(released.contains("Claim released."), "{released}");
    let taken = claim(&s, &second, &issue);
    assert!(taken.contains("Claim held"), "{taken}");
}

#[test]
fn a_claim_lapses_when_its_time_passes_with_no_job_running() {
    let s = setup();
    let (first, second) = (session(&s), session(&s));
    let issue = create(&s, &first, "Lapsing", "todo");
    {
        let _now = queries::claims::pin_now("2026-11-26 12:00:00");
        claim(&s, &first, &issue);
    }
    {
        let _now = queries::claims::pin_now("2026-11-26 12:14:59");
        assert!(claim(&s, &second, &issue).starts_with("Error:"));
        assert_eq!(workable(&s, &second), "No issues found.");
    }
    let _now = queries::claims::pin_now("2026-11-26 12:15:00");
    assert!(workable(&s, &second).contains("Lapsing"));
    assert!(!get(&s, &second, &issue).contains("Claimed by"));
    let taken = claim(&s, &second, &issue);
    assert!(taken.contains("Claim held"), "{taken}");
}

#[test]
fn a_reclaim_extends_the_claim_and_keeps_when_it_began() {
    let s = setup();
    let m = session(&s);
    let issue = create(&s, &m, "Heartbeat", "active");
    {
        let _now = queries::claims::pin_now("2026-11-26 12:00:00");
        claim(&s, &m, &issue);
    }
    let (_, _, _, seq_before) = stored(&s, &issue);
    let _now = queries::claims::pin_now("2026-11-26 12:10:00");
    let renewed = update(
        &s,
        &m,
        UpdateIssueInput {
            identifier: issue.clone(),
            claim: Some(true),
            claim_minutes: Some(60),
            ..Default::default()
        },
    );
    assert!(
        renewed.contains("Claim renewed until 2026-11-26 13:10 UTC"),
        "{renewed}"
    );
    let (claimed_at, expires_at, status, seq_after) = stored(&s, &issue);
    assert_eq!(claimed_at, "2026-11-26 12:00:00");
    assert_eq!(expires_at, "2026-11-26 13:10:00");
    assert_eq!(status, "active");
    assert_eq!(seq_after, seq_before, "a heartbeat writes no issue row");
}

#[test]
fn get_issue_and_list_rows_name_the_holder() {
    let s = setup();
    let (first, second) = (session(&s), session(&s));
    let issue = create(&s, &first, "Named holder", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    claim(&s, &first, &issue);
    let worker = first.session_worker().unwrap();
    let holder = format!("@maintainer (worker {worker}) until 2026-11-26 12:15 UTC");

    let own = get(&s, &first, &issue);
    assert!(own.contains(&format!("Claimed by {holder} (you)")), "{own}");
    let other = get(&s, &second, &issue);
    assert!(other.contains(&format!("Claimed by {holder}\n")), "{other}");

    let rows = as_user(&s.maintainer, || {
        second
            .list_issues(Parameters(ListIssuesInput {
                project: Some("MEM".into()),
                ..Default::default()
            }))
            .text
    });
    assert!(rows.contains(&format!("| claimed by {holder}")), "{rows}");
}

#[test]
fn a_new_session_resumes_a_claim_by_passing_its_worker_back() {
    let s = setup();
    let (first, restarted) = (session(&s), session(&s));
    let issue = create(&s, &first, "Resumed", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    claim(&s, &first, &issue);
    let worker = first.session_worker().unwrap();

    let resumed = update(
        &s,
        &restarted,
        UpdateIssueInput {
            identifier: issue,
            claim: Some(true),
            worker: Some(worker.clone()),
            ..Default::default()
        },
    );
    assert!(resumed.contains("Claim renewed"), "{resumed}");
    // The restarted process now works as that worker.
    assert_eq!(restarted.session_worker().as_deref(), Some(worker.as_str()));
    assert!(workable(&s, &restarted).contains("Resumed"));
}

#[test]
fn a_different_account_is_refused_and_cannot_release() {
    let s = setup();
    let m = session(&s);
    let issue = create(&s, &m, "Someone else's", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    claim(&s, &m, &issue);
    let lead_release = as_user(&s.lead, || {
        m.update_issue(Parameters(UpdateIssueInput {
            identifier: issue.clone(),
            claim: Some(false),
            ..Default::default()
        }))
    });
    assert!(
        lead_release.contains("only its holder can release it"),
        "{lead_release}"
    );
}

#[test]
fn a_refused_claim_changes_nothing_else_in_the_call() {
    let s = setup();
    let (first, second) = (session(&s), session(&s));
    let issue = create(&s, &first, "Untouched", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    claim(&s, &first, &issue);
    let refused = update(
        &s,
        &second,
        UpdateIssueInput {
            identifier: issue.clone(),
            title: Some("Renamed".into()),
            claim: Some(true),
            ..Default::default()
        },
    );
    assert!(refused.starts_with("Error:"), "{refused}");
    assert!(get(&s, &first, &issue).contains("Untouched"));
}

#[test]
fn claim_parameters_are_checked() {
    let s = setup();
    let m = session(&s);
    let issue = create(&s, &m, "Checked", "todo");
    let call = |input: UpdateIssueInput| {
        update(
            &s,
            &m,
            UpdateIssueInput {
                identifier: issue.clone(),
                ..input
            },
        )
    };
    let out = call(UpdateIssueInput {
        claim_minutes: Some(30),
        ..Default::default()
    });
    assert!(out.contains("claim_minutes needs claim=true"), "{out}");
    let out = call(UpdateIssueInput {
        worker: Some("p-1".into()),
        ..Default::default()
    });
    assert!(out.contains("worker needs claim"), "{out}");
    let out = call(UpdateIssueInput {
        claim: Some(true),
        claim_minutes: Some(24 * 60 + 1),
        ..Default::default()
    });
    assert!(out.contains("claim_minutes must be 1 to 1440"), "{out}");
    let out = call(UpdateIssueInput {
        claim: Some(true),
        worker: Some("not a worker".into()),
        ..Default::default()
    });
    assert!(out.contains("invalid worker"), "{out}");
    let out = call(UpdateIssueInput {
        claim: Some(true),
        status: Some("done".into()),
        ..Default::default()
    });
    assert!(out.contains("only open issues can be claimed"), "{out}");
    let out = call(UpdateIssueInput {
        claim: Some(false),
        ..Default::default()
    });
    assert!(out.contains("No live claim to release."), "{out}");
}

#[test]
fn closing_an_issue_ends_its_claim() {
    let s = setup();
    let m = session(&s);
    let issue = create(&s, &m, "Finished", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    claim(&s, &m, &issue);
    update(
        &s,
        &m,
        UpdateIssueInput {
            identifier: issue.clone(),
            status: Some("done".into()),
            ..Default::default()
        },
    );
    assert!(!get(&s, &m, &issue).contains("Claimed by"));
}

#[test]
fn a_stateless_http_call_claims_for_the_account() {
    let s = setup();
    let http = LificMcp::with_realtime(s.db.clone(), crate::realtime::RealtimeHub::new());
    let issue = create(&s, &http, "Account claim", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    let held = claim(&s, &http, &issue);
    assert!(held.contains("for your whole account"), "{held}");

    // A session header makes a worker of its own, which the account claim
    // counts as one of its own sessions.
    let in_session = |session: &str, f: &dyn Fn() -> String| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(crate::mcp::with_request_user(
                Some(s.maintainer.clone()),
                || {
                    crate::mcp::with_request_worker(
                        crate::db::queries::claims::worker_from_session(session),
                        async { f() },
                    )
                },
            ))
    };
    let list = || {
        http.list_issues(Parameters(ListIssuesInput {
            project: Some("MEM".into()),
            workable: Some(true),
            ..Default::default()
        }))
        .text
    };
    assert!(in_session("a", &list).contains("Account claim"));
}

#[test]
fn the_briefing_offers_only_what_nobody_else_holds() {
    let s = setup();
    let (first, second) = (session(&s), session(&s));
    let issue = create(&s, &first, "Held for later", "todo");
    create(&s, &first, "Free to take", "todo");
    let _now = queries::claims::pin_now("2026-11-26 12:00:00");
    // Claim with an explicit status, so the issue stays todo and would
    // otherwise sit in the workable section.
    update(
        &s,
        &first,
        UpdateIssueInput {
            identifier: issue,
            claim: Some(true),
            status: Some("todo".into()),
            ..Default::default()
        },
    );
    let briefing = |m: &LificMcp| {
        as_user(&s.maintainer, || {
            m.get_briefing(Parameters(GetBriefingInput {
                project: Some("MEM".into()),
                ..Default::default()
            }))
            .text
        })
    };
    let other = briefing(&second);
    let workable = other
        .split("Workable")
        .nth(1)
        .expect("a workable section")
        .to_string();
    assert!(workable.contains("Free to take"), "{other}");
    assert!(!workable.contains("Held for later"), "{other}");
    let own = briefing(&first);
    assert!(own.contains("Held for later"), "{own}");
    assert!(own.contains("claimed by @maintainer"), "{own}");
}
