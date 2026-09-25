//! LIF-483: `get_briefing` sections, cursor handling, budget, and gates.

use super::super::tests::{mcp, seed_project, setup_membership_mcp};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn issue(m: &LificMcp, project: &str, title: &str, status: &str, priority: &str) {
    let created = m.create_issue(Parameters(CreateIssueInput {
        project: Some(project.into()),
        title: title.into(),
        status: Some(status.into()),
        priority: Some(priority.into()),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
}

fn set_status(m: &LificMcp, identifier: &str, status: &str) {
    let updated = m.update_issue(Parameters(UpdateIssueInput {
        identifier: identifier.into(),
        status: Some(status.into()),
        ..Default::default()
    }));
    assert!(!updated.starts_with("Error"), "got: {updated}");
}

fn page(m: &LificMcp, project: &str, title: &str, content: &str) {
    let created = m.create_page(Parameters(CreatePageInput {
        project: Some(project.into()),
        title: title.into(),
        content: Some(content.into()),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
}

fn briefing(m: &LificMcp, project: Option<&str>, since: Option<&str>, pages: &[&str]) -> String {
    m.get_briefing(Parameters(GetBriefingInput {
        project: project.map(Into::into),
        since: since.map(Into::into),
        pages: (!pages.is_empty()).then(|| pages.iter().map(|page| page.to_string()).collect()),
    }))
}

/// Everything recorded so far moves two hours back, and the returned cursor
/// sits one hour back: whatever happens next is "since" and nothing before
/// it is. Timestamps have one-second resolution, so this beats sleeping.
/// Moving `seq` in the same statement keeps the `updated_at` bump triggers,
/// which only fire when `seq` is unchanged, from undoing the backdate.
fn backdate_and_take_cursor(m: &LificMcp) -> String {
    m.write(|conn| {
        conn.execute_batch(
            "UPDATE audit_log SET ts = datetime('now', '-2 hours');
             UPDATE issues SET created_at = datetime('now', '-2 hours'),
                               updated_at = datetime('now', '-2 hours'),
                               seq = seq + 1000000;
             UPDATE pages SET created_at = datetime('now', '-2 hours'),
                              updated_at = datetime('now', '-2 hours'),
                              seq = seq + 1000000;",
        )?;
        Ok(conn.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now', '-1 hour')",
            [],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

/// The section of `output` that starts at the heading beginning `heading`.
fn section<'a>(output: &'a str, heading: &str) -> &'a str {
    let start = output
        .find(&format!("\n## {heading}"))
        .unwrap_or_else(|| panic!("no '{heading}' section in:\n{output}"));
    let rest = &output[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |end| end + 3);
    &rest[..end]
}

/// A project with one of everything the briefing reports.
fn seeded() -> (LificMcp, McpTestGuard, String) {
    let (m, guard) = mcp();
    seed_project(&m, "Briefing", "BRF");
    issue(&m, "BRF", "Blocker work", "todo", "high"); // BRF-1
    issue(&m, "BRF", "Blocked work", "todo", "urgent"); // BRF-2
    issue(&m, "BRF", "Workable urgent", "todo", "urgent"); // BRF-3
    issue(&m, "BRF", "In flight", "active", "medium"); // BRF-4
    issue(&m, "BRF", "Old work", "todo", "low"); // BRF-5
    let linked = m.link_issues(Parameters(LinkIssuesInput {
        source: "BRF-1".into(),
        target: "BRF-2".into(),
        relation_type: "blocks".into(),
        ..Default::default()
    }));
    assert!(!linked.starts_with("Error"), "got: {linked}");
    let plan = m.create_plan(Parameters(CreatePlanInput {
        project: Some("BRF".into()),
        title: "Ship it".into(),
        anchor_issue: None,
        steps: Some(vec![
            PlanStepInput {
                title: "Design".into(),
                done: Some(true),
                ..Default::default()
            },
            PlanStepInput {
                title: "Build".into(),
                steps: Some(vec![PlanStepInput {
                    title: "Wire API".into(),
                    issue: Some("BRF-3".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        ]),
    }));
    assert!(plan.starts_with("Created"), "got: {plan}");
    page(&m, "BRF", "Canon", "The rules.");
    page(&m, "BRF", "Notes", "Scratch.");

    let cursor = backdate_and_take_cursor(&m);
    issue(&m, "BRF", "Fresh idea", "backlog", "none"); // BRF-6
    set_status(&m, "BRF-5", "done");
    set_status(&m, "BRF-1", "active");
    let edited = m.update_page(Parameters(UpdatePageInput {
        identifier: "BRF-DOC-1".into(),
        content: Some("The rules, revised.".into()),
        ..Default::default()
    }));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    (m, guard, cursor)
}

#[test]
fn a_briefing_reports_every_section_with_its_items_since_the_cursor() {
    let (m, _guard, cursor) = seeded();
    let out = briefing(
        &m,
        Some("BRF"),
        Some(&cursor),
        &["BRF-DOC-1", "BRF-DOC-2", "BRF-DOC-99"],
    );
    assert!(out.starts_with("BRF briefing at "), "got: {out}");
    assert!(out.chars().count() <= BUDGET_CHARS, "{out}");

    let changes = section(&out, "Since ");
    assert!(
        changes.contains("Issues created 1, closed 1, otherwise moved 1. Pages edited 1."),
        "{changes}"
    );
    assert!(
        changes.contains("closed BRF-5 (done) | Old work"),
        "{changes}"
    );
    assert!(
        changes.contains("new BRF-6 | backlog | none | Fresh idea"),
        "{changes}"
    );
    assert!(
        changes.contains("moved BRF-1 todo -> active | Blocker work"),
        "{changes}"
    );
    assert!(changes.contains("page BRF-DOC-1 Canon"), "{changes}");
    assert!(!changes.contains("Notes"), "unchanged page: {changes}");
    assert!(!changes.contains("BRF-2"), "untouched issue: {changes}");

    let plans = section(&out, "Active plans (1)");
    assert!(
        plans.contains("BRF-PLAN-1 Ship it (1/3 done), next: #"),
        "{plans}"
    );
    assert!(plans.contains(" Wire API [BRF-3]"), "{plans}");

    let blocked = section(&out, "Blocked (1)");
    assert!(
        blocked.contains("- BRF-2 | todo | urgent | Blocked work; blocked by BRF-1 (active)"),
        "{blocked}"
    );

    // Priority first; blocked, active and closed issues are not workable here.
    let workable = section(&out, "Workable, not yet active (2)");
    let lines: Vec<&str> = workable
        .lines()
        .filter(|line| line.starts_with("- "))
        .collect();
    assert_eq!(
        lines,
        [
            "- BRF-3 | todo | urgent | Workable urgent",
            "- BRF-6 | backlog | none | Fresh idea",
        ],
        "{workable}"
    );

    let pages = section(&out, "Key pages");
    assert!(pages.contains("- BRF-DOC-1 Canon | seq "), "{pages}");
    assert!(
        pages.contains("| 19 chars | changed since cursor"),
        "{pages}"
    );
    assert!(
        pages.contains("Notes | seq ") && pages.contains("| 8 chars | unchanged since cursor"),
        "{pages}"
    );
    assert!(pages.contains("- BRF-DOC-99: not found"), "{pages}");
    assert!(!pages.contains("The rules"), "no page bodies: {pages}");

    let active = section(&out, "Active (2)");
    assert!(
        active.contains("- BRF-1 | active | high | Blocker work"),
        "{active}"
    );
    assert!(
        active.contains("- BRF-4 | active | medium | In flight"),
        "{active}"
    );
}

#[test]
fn without_since_there_are_no_changes_and_recent_pages_stand_in_for_named_ones() {
    let (m, _guard, _cursor) = seeded();
    let out = briefing(&m, Some("BRF"), None, &[]);
    assert!(!out.contains("## Since"), "{out}");
    let pages = section(&out, "Recently updated pages");
    // The edited page is the most recent one.
    let first = pages.lines().nth(1).unwrap_or_default();
    assert!(first.starts_with("- BRF-DOC-1 Canon | seq "), "{pages}");
    assert!(first.contains(" UTC"), "{pages}");
    assert!(pages.contains("BRF-DOC-2 Notes"), "{pages}");
    assert!(out.contains("Resume later with since='"), "{out}");
}

#[test]
fn a_cursor_after_everything_reports_no_changes_and_omits_the_section() {
    let (m, _guard, _cursor) = seeded();
    let out = briefing(&m, Some("BRF"), Some("2999-01-01"), &[]);
    assert!(
        out.contains("No changes since 2999-01-01 00:00:00 UTC."),
        "{out}"
    );
    assert!(!out.contains("## Since"), "{out}");
    assert!(out.contains("## Active plans"), "{out}");
}

#[test]
fn an_empty_project_omits_every_section() {
    let (m, _guard) = mcp();
    seed_project(&m, "Quiet", "QUI");
    let out = briefing(&m, Some("QUI"), None, &[]);
    assert!(!out.contains("##"), "{out}");
    assert!(
        out.contains("Nothing is planned, active, blocked or workable."),
        "{out}"
    );
}

#[test]
fn a_large_project_stays_under_the_budget_and_says_what_it_trimmed() {
    let (m, _guard) = mcp();
    seed_project(&m, "Crowded", "BIG");
    let long = "x".repeat(150);
    for index in 0..30 {
        issue(
            &m,
            "BIG",
            &format!("Active {index} {long}"),
            "active",
            "high",
        );
        issue(&m, "BIG", &format!("Todo {index} {long}"), "todo", "medium");
    }
    for index in 0..7 {
        let plan = m.create_plan(Parameters(CreatePlanInput {
            project: Some("BIG".into()),
            title: format!("Plan {index} {long}"),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: format!("Step {long}"),
                ..Default::default()
            }]),
        }));
        assert!(plan.starts_with("Created"), "got: {plan}");
    }
    let cursor = backdate_and_take_cursor(&m);
    for index in 0..12 {
        issue(&m, "BIG", &format!("New {index} {long}"), "backlog", "low");
    }

    let out = briefing(&m, Some("BIG"), Some(&cursor), &[]);
    assert!(
        out.chars().count() <= BUDGET_CHARS,
        "{} chars:\n{out}",
        out.chars().count()
    );
    for heading in ["Since ", "Active plans (7)", "Workable", "Active (30)"] {
        section(&out, heading);
    }
    assert!(
        out.contains("more: list_issues(project='BIG', status='active'))"),
        "{out}"
    );
    assert!(
        out.contains("more: list_resources(resource_type='plan', project='BIG'))"),
        "{out}"
    );
    assert!(
        out.contains("more: get_activity(identifier='BIG', since='"),
        "{out}"
    );
    assert!(out.contains('…'), "long titles are shortened: {out}");
}

#[test]
fn a_bound_session_briefs_the_bound_project_and_an_unbound_one_asks_for_it() {
    let (m, _guard) = mcp();
    seed_project(&m, "Bound", "BND");
    issue(&m, "BND", "Bound work", "active", "high");
    let unbound = briefing(&m, None, None, &[]);
    assert!(unbound.contains("project required"), "{unbound}");

    let bound = m.with_bound_project(Some("BND".into()));
    let out = briefing(&bound, None, None, &[]);
    assert!(out.starts_with("BND briefing at "), "{out}");
    assert!(out.contains("BND-1 | active | high | Bound work"), "{out}");
}

#[test]
fn an_unparseable_since_is_an_error() {
    let (m, _guard) = mcp();
    seed_project(&m, "Quiet", "QUI");
    let out = briefing(&m, Some("QUI"), Some("soon"), &[]);
    assert!(out.starts_with("Error: "), "{out}");
    assert!(out.contains("invalid since"), "{out}");
}

// ── Authorization ────────────────────────────────────────────────────

fn as_user(user: &models::AuthUser, f: impl FnOnce() -> String) -> String {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_user(
            Some(user.clone()),
            || async { f() },
        ))
}

#[test]
fn a_briefing_is_gated_like_list_issues_and_never_names_invisible_records() {
    let (m, _admin, lead, _maintainer, viewer, non_member, _project_id, _guard) =
        setup_membership_mcp();
    let created = as_user(&lead, || {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Held up".into(),
            ..Default::default()
        }))
    });
    assert!(created.starts_with("Created"), "got: {created}");

    // A project none of the fixture's members belong to, with an issue that
    // blocks MEM-1 and a page of its own.
    {
        let conn = m.db.write().unwrap();
        let foreign = queries::create_project(
            &conn,
            &models::CreateProject {
                name: "Foreign".into(),
                identifier: "FGN".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let blocker = queries::create_issue(
            &conn,
            &models::CreateIssue {
                project_id: foreign.id,
                title: "Classified blocker".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let target = queries::resolve_identifier(&conn, "MEM-1").unwrap();
        queries::link_issues(&conn, blocker.id, target, "blocks").unwrap();
        queries::create_page(
            &conn,
            &models::CreatePage {
                project_id: Some(foreign.id),
                title: "Classified page".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }

    let denied = as_user(&non_member, || briefing(&m, Some("MEM"), None, &[]));
    assert!(denied.starts_with("Error: Forbidden:"), "{denied}");
    let foreign = as_user(&viewer, || briefing(&m, Some("FGN"), None, &[]));
    assert!(foreign.starts_with("Error: Forbidden:"), "{foreign}");

    let out = as_user(&viewer, || {
        briefing(&m, Some("MEM"), None, &["FGN-DOC-1", "FGN-DOC-404"])
    });
    let blocked = section(&out, "Blocked (1)");
    assert!(
        blocked.contains("Held up; blocked by 1 in a project you cannot view"),
        "{blocked}"
    );
    assert!(!out.contains("FGN-1"), "{out}");
    assert!(!out.contains("Classified"), "{out}");
    let pages = section(&out, "Key pages");
    // Hidden and missing pages read the same.
    assert!(pages.contains("- FGN-DOC-1: not found"), "{pages}");
    assert!(pages.contains("- FGN-DOC-404: not found"), "{pages}");
}

#[test]
fn the_budget_pass_trims_the_longest_sections_and_notes_each_cut() {
    let make = |heading: &str, lines: usize| Section {
        heading: heading.into(),
        summary: None,
        lines: (0..lines)
            .map(|index| format!("{heading} line {index} {}", "y".repeat(180)))
            .collect(),
        total: lines,
        total_is_floor: false,
        more: format!("more_{heading}()"),
    };
    let mut sections = vec![make("short", 2), make("long", 40), make("middle", 20)];
    let out = fit_to_budget("header\n", &mut sections).unwrap();

    assert!(
        out.chars().count() <= BUDGET_CHARS,
        "{}",
        out.chars().count()
    );
    assert_eq!(sections[0].lines.len(), 2, "the short section is untouched");
    assert!(
        sections[1].lines.len().abs_diff(sections[2].lines.len()) <= 1,
        "the longest sections are cut evenly: {} and {}",
        sections[1].lines.len(),
        sections[2].lines.len()
    );
    assert!(!out.contains("more_short"), "{out}");
    assert!(
        out.contains(&format!(
            "(+{} more: more_long())",
            40 - sections[1].lines.len()
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "(+{} more: more_middle())",
            20 - sections[2].lines.len()
        )),
        "{out}"
    );
}

#[test]
fn briefing_names_holding_waits_as_blockers_and_lists_due_date_waits() {
    let (m, _guard) = mcp();
    let _day = crate::db::queries::waits::pin_today("2026-09-25");
    seed_project(&m, "Waits", "WTB");
    issue(&m, "WTB", "Needs a decision", "todo", "high"); // WTB-1
    issue(&m, "WTB", "Filing pending", "todo", "medium"); // WTB-2
    issue(&m, "WTB", "Bank reply", "todo", "medium"); // WTB-3
    issue(&m, "WTB", "Vendor quote", "todo", "low"); // WTB-4
    let wait = |target: &str, input: LinkIssuesInput| {
        let added = m.link_issues(Parameters(LinkIssuesInput {
            target: target.into(),
            relation_type: "blocks".into(),
            ..input
        }));
        assert!(!added.starts_with("Error"), "got: {added}");
    };
    wait(
        "WTB-1",
        LinkIssuesInput {
            user: Some("admin".into()),
            note: Some("pick the schema".into()),
            ..Default::default()
        },
    );
    wait(
        "WTB-2",
        LinkIssuesInput {
            from: Some("2026-09-28".into()),
            until: Some("2026-09-29".into()),
            ..Default::default()
        },
    );
    wait(
        "WTB-3",
        LinkIssuesInput {
            from: Some("2026-09-24".into()),
            until: Some("2026-09-26".into()),
            note: Some("bank said 2 days".into()),
            ..Default::default()
        },
    );
    wait(
        "WTB-4",
        LinkIssuesInput {
            from: Some("2026-09-20".into()),
            ..Default::default()
        },
    );

    let out = briefing(&m, Some("WTB"), None, &[]);
    let due = out.split("## Due to check (2)").nth(1).expect(&out);
    let due = due.split("\n## ").next().unwrap();
    let overdue_at = due.find("WTB-4").expect(due);
    let due_at = due.find("WTB-3").expect(due);
    assert!(overdue_at < due_at, "most overdue first: {due}");
    assert!(due.contains("Overdue since 2026-09-21"), "{due}");
    assert!(
        due.contains("Due to check since 2026-09-24, expected by 2026-09-26 (bank said 2 days)"),
        "{due}"
    );
    assert!(!due.contains("WTB-1") && !due.contains("WTB-2"), "{due}");

    let blocked = out.split("## Blocked (2)").nth(1).expect(&out);
    let blocked = blocked.split("\n## ").next().unwrap();
    assert!(
        blocked.contains("; waiting on @admin (pick the schema)"),
        "{blocked}"
    );
    assert!(
        blocked.contains("; waiting until 2026-09-28..29"),
        "{blocked}"
    );
    assert!(
        !blocked.contains("blocked by"),
        "no empty issue-blocker clause: {blocked}"
    );
}
