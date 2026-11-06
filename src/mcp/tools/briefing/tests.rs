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

/// Review fix: a plan step can mirror an issue in another project (an admin
/// can link across projects), and the next-step line used to print that
/// issue's identifier to anyone who can view the plan's project.
#[test]
fn a_next_step_linked_into_an_invisible_project_is_not_named() {
    let (m, admin, _lead, _maintainer, viewer, _non_member, project_id, _guard) =
        setup_membership_mcp();
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
        let hidden = queries::create_issue(
            &conn,
            &models::CreateIssue {
                project_id: foreign.id,
                title: "Classified step issue".into(),
                ..Default::default()
            },
        )
        .unwrap();
        queries::plans::create_plan(
            &conn,
            &models::CreatePlan {
                project_id,
                title: "Cross-project plan".into(),
                issue_id: None,
                steps: vec![models::CreatePlanStep {
                    title: "Mirror it".into(),
                    description: String::new(),
                    issue_id: Some(hidden.id),
                    done: false,
                    steps: Vec::new(),
                }],
            },
        )
        .unwrap();
    }

    let out = as_user(&viewer, || briefing(&m, Some("MEM"), None, &[]));
    let plans = section(&out, "Active plans (1)");
    assert!(
        plans.contains("Mirror it [an issue in a project you cannot view]"),
        "{plans}"
    );
    assert!(!out.contains("FGN-"), "{out}");

    // Someone who can see the linked issue still gets its identifier.
    let out = as_user(&admin, || briefing(&m, Some("MEM"), None, &[]));
    assert!(out.contains("Mirror it [FGN-1]"), "{out}");
}

// ── since='last' (LIF-490) ──────────────────────────────────────────

fn comment(m: &LificMcp, identifier: &str, content: &str) {
    let added = m.add_comment(Parameters(AddCommentInput {
        identifier: identifier.into(),
        content: content.into(),
    }));
    assert!(!added.starts_with("Error"), "got: {added}");
}

/// Two briefings in a row: the second reports exactly what happened in
/// between, with who did it, and nothing from before the first. Both run in
/// the same second, which a timestamp cursor could not tell apart.
#[test]
fn since_last_reports_only_the_changes_after_the_callers_previous_briefing() {
    let (m, _admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    as_user(&lead, || {
        issue(&m, "MEM", "Before the first briefing", "todo", "high");
        String::new()
    });

    let first = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(
        first.contains("No earlier briefing of MEM for you to resume from"),
        "{first}"
    );
    assert!(!first.contains("## Since"), "{first}");
    assert!(first.contains("Resume later with since='last'."), "{first}");

    as_user(&lead, || {
        issue(&m, "MEM", "Between briefings", "backlog", "low");
        set_status(&m, "MEM-1", "active");
        comment(&m, "MEM-1", "Picked this up.\nMore detail below.");
        String::new()
    });

    let second = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    let changes = section(&second, "Since your last briefing (");
    let by = format!(" by @{}", lead.username);
    assert!(
        changes
            .contains("Issues created 1, closed 0, otherwise moved 1. Pages edited 0. Comments 1."),
        "{changes}"
    );
    assert!(
        changes.contains(&format!(
            "new MEM-2 | backlog | low | Between briefings{by}"
        )),
        "{changes}"
    );
    assert!(
        changes.contains(&format!(
            "moved MEM-1 todo -> active{by} | Before the first briefing"
        )),
        "{changes}"
    );
    assert!(
        changes.contains(&format!("comment on MEM-1{by}: Picked this up.")),
        "{changes}"
    );
    assert!(
        !changes.contains("More detail"),
        "first line only: {changes}"
    );
    assert!(!changes.contains("new MEM-1"), "{changes}");

    // Nothing happened since the second briefing.
    let third = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(!third.contains("## Since"), "{third}");
    assert!(
        third.contains("No changes since your last briefing ("),
        "{third}"
    );
}

/// The cursor belongs to the caller and the project: one person's briefing
/// does not use up another's changes, nor one project's another's.
#[test]
fn each_caller_and_project_keeps_its_own_cursor() {
    let (m, admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    as_user(&admin, || {
        seed_project(&m, "Other", "OTH");
        String::new()
    });
    for user in [&viewer, &lead] {
        as_user(user, || briefing(&m, Some("MEM"), Some("last"), &[]));
    }
    as_user(&admin, || briefing(&m, Some("OTH"), Some("last"), &[]));

    as_user(&lead, || {
        issue(&m, "MEM", "Fresh", "todo", "none");
        String::new()
    });

    // The lead looks first; the viewer's cursor is untouched by that.
    let lead_view = as_user(&lead, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(lead_view.contains("new MEM-1"), "{lead_view}");
    let viewer_view = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(viewer_view.contains("new MEM-1"), "{viewer_view}");

    // A briefing of another project neither reports nor consumes MEM's.
    let other = as_user(&admin, || briefing(&m, Some("OTH"), Some("last"), &[]));
    assert!(!other.contains("MEM-1"), "{other}");
    let admin_first = as_user(&admin, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(
        admin_first.contains("No earlier briefing of MEM for you to resume from"),
        "{admin_first}"
    );
}

/// A briefing without `since`, or with a timestamp, still moves the cursor:
/// `since='last'` means the caller's last briefing of any kind.
#[test]
fn any_briefing_moves_the_cursor_and_a_timestamp_since_also_names_actors() {
    let (m, _admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    as_user(&viewer, || briefing(&m, Some("MEM"), None, &[]));
    as_user(&lead, || {
        issue(&m, "MEM", "Seen by a timestamp", "todo", "none");
        comment(&m, "MEM-1", "First note");
        String::new()
    });

    let by_time = as_user(&viewer, || {
        briefing(&m, Some("MEM"), Some("2000-01-01"), &[])
    });
    let changes = section(&by_time, "Since 2000-01-01 00:00:00 UTC");
    assert!(
        changes.contains(&format!(
            "comment on MEM-1 by @{}: First note",
            lead.username
        )),
        "{changes}"
    );

    let resumed = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(!resumed.contains("## Since"), "already reported: {resumed}");
}

/// Changes to deleted records and to other projects stay out of the section.
#[test]
fn since_last_skips_deleted_records_and_other_projects() {
    let (m, admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    as_user(&admin, || {
        seed_project(&m, "Other", "OTH");
        String::new()
    });
    as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    as_user(&lead, || {
        issue(&m, "MEM", "Short lived", "todo", "none");
        String::new()
    });
    as_user(&admin, || {
        issue(&m, "OTH", "Elsewhere", "todo", "none");
        comment(&m, "OTH-1", "Not yours");
        let deleted = m.delete(Parameters(DeleteInput {
            resource_type: "issue".into(),
            identifier: "MEM-1".into(),
            project: None,
        }));
        assert!(!deleted.starts_with("Error"), "got: {deleted}");
        String::new()
    });

    let out = as_user(&viewer, || briefing(&m, Some("MEM"), Some("last"), &[]));
    assert!(!out.contains("## Since"), "{out}");
    assert!(!out.contains("OTH-1"), "{out}");
}

/// Review fix: sections used to read the first 100 rows by priority and only
/// then drop the ones they exclude, so enough higher-priority excluded rows
/// hid every eligible issue and the section vanished without a word.
#[test]
fn excluded_rows_ahead_of_an_eligible_issue_do_not_hide_it() {
    let (m, _guard) = mcp();
    seed_project(&m, "Crowd", "CRW");
    let project_id = m
        .read(|conn| queries::resolve_project_identifier(conn, "CRW"))
        .unwrap();
    m.write(|conn| {
        let create = |title: String, status, priority| {
            queries::create_issue(
                conn,
                &models::CreateIssue {
                    project_id,
                    title,
                    status,
                    priority,
                    ..Default::default()
                },
            )
        };
        let blocker = create(
            "Blocker".into(),
            models::Status::Todo,
            models::Priority::None,
        )?;
        for index in 0..105 {
            // Active and urgent: ahead of everything workable.
            create(
                format!("Busy {index}"),
                models::Status::Active,
                models::Priority::Urgent,
            )?;
            // Closed but still carrying an open blocker: ahead of the open one.
            let closed = create(
                format!("Closed {index}"),
                models::Status::Done,
                models::Priority::Urgent,
            )?;
            queries::link_issues(conn, blocker.id, closed.id, "blocks")?;
        }
        create(
            "Pick me up".into(),
            models::Status::Todo,
            models::Priority::Low,
        )?;
        let open = create(
            "Still stuck".into(),
            models::Status::Todo,
            models::Priority::Low,
        )?;
        queries::link_issues(conn, blocker.id, open.id, "blocks")?;
        Ok(())
    })
    .unwrap();

    let out = briefing(&m, Some("CRW"), None, &[]);
    let workable = section(&out, "Workable, not yet active");
    assert!(workable.contains("Pick me up"), "{workable}");
    assert!(!workable.contains("Busy"), "{workable}");
    let blocked = section(&out, "Blocked (1)");
    assert!(blocked.contains("Still stuck"), "{blocked}");
    assert!(!blocked.contains("Closed"), "{blocked}");
    // A count cut short by the scan says so rather than posing as exact.
    section(&out, "Active (100+)");
}

// ── LIF-491: the human queue ─────────────────────────────────────────

fn human_issue(m: &LificMcp, project: &str, title: &str, status: &str, due: Option<&str>) {
    let created = m.create_issue(Parameters(CreateIssueInput {
        project: Some(project.into()),
        title: title.into(),
        status: Some(status.into()),
        target_date: due.map(Into::into),
        assignees: Some(vec!["human".into()]),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
}

fn blocks(m: &LificMcp, source: &str, target: &str) {
    let linked = m.link_issues(Parameters(LinkIssuesInput {
        source: source.into(),
        target: target.into(),
        relation_type: "blocks".into(),
        ..Default::default()
    }));
    assert!(!linked.starts_with("Error"), "got: {linked}");
}

#[test]
fn the_briefing_opens_with_the_human_queue_ranked_by_blocking_then_due_then_age() {
    let (m, _guard) = mcp();
    seed_project(&m, "Queue A", "HQA");
    seed_project(&m, "Queue B", "HQB");
    human_issue(&m, "HQA", "Sign the lease", "todo", None); // HQA-1
    human_issue(&m, "HQA", "Renew domain", "todo", None); // HQA-2
    human_issue(&m, "HQA", "Pick a logo", "backlog", Some("2026-11-20")); // HQA-3
    issue(&m, "HQA", "Agent work", "todo", "high"); // HQA-4
    human_issue(&m, "HQA", "Call the bank", "todo", None); // HQA-5
    human_issue(&m, "HQA", "Already handled", "done", None); // HQA-6
    issue(&m, "HQA", "Agent blocker", "todo", "low"); // HQA-7
    human_issue(&m, "HQB", "Renew  Domain", "todo", Some("2026-12-01")); // HQB-1
    human_issue(&m, "HQB", "Approve budget", "active", None); // HQB-2
    issue(&m, "HQB", "Spend it", "todo", "medium"); // HQB-3
    issue(&m, "HQB", "Report it", "backlog", "medium"); // HQB-4
    issue(&m, "HQB", "Shipped", "done", "medium"); // HQB-5
    blocks(&m, "HQB-2", "HQB-3");
    blocks(&m, "HQB-2", "HQB-4");
    // A closed issue is no longer held up, so it does not count.
    blocks(&m, "HQB-2", "HQB-5");
    blocks(&m, "HQA-2", "HQA-4");
    // An agent's blocker is not in the queue at all.
    blocks(&m, "HQA-7", "HQA-1");
    // Older than HQA-1 despite the later sequence.
    m.write(|conn| {
        conn.execute(
            "UPDATE issues SET created_at = datetime('now', '-3 days'), seq = seq + 1000000
              WHERE sequence = 5 AND project_id = (SELECT id FROM projects WHERE identifier = 'HQA')",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let out = briefing(&m, Some("HQA"), None, &[]);
    let first = out.find("\n## ").expect("a section");
    assert!(
        out[first..].starts_with("\n## Human queue (6), every project"),
        "the queue leads: {out}"
    );
    let queue = section(&out, "Human queue");
    assert!(queue.contains("By project: HQA 4, HQB 2."), "{queue}");
    let order: Vec<&str> = queue
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|line| line.split(" | ").next().unwrap())
        .collect();
    assert_eq!(
        order,
        ["HQB-2", "HQA-2", "HQA-3", "HQB-1", "HQA-5", "HQA-1"],
        "{queue}"
    );
    assert!(
        queue.contains("- HQB-2 | active | none | Approve budget | for human | blocks 2 | opened "),
        "{queue}"
    );
    assert!(
        queue.contains(
            "- HQA-3 | backlog | none | Pick a logo | for human | due 2026-11-20 | opened "
        ),
        "{queue}"
    );
    let line = |id: &str| {
        queue
            .lines()
            .find(|line| line.starts_with(&format!("- {id} ")))
            .unwrap()
    };
    assert!(
        line("HQA-2").starts_with("- HQA-2 | todo | none | Renew domain | for human | blocks 1 |")
            && line("HQA-2").ends_with(" | possible duplicate of HQB-1"),
        "{queue}"
    );
    assert!(
        line("HQB-1").contains(" | due 2026-12-01 | ")
            && line("HQB-1").ends_with(" | possible duplicate of HQA-2"),
        "{queue}"
    );
    let plain = queue
        .lines()
        .find(|line| line.starts_with("- HQA-1 "))
        .unwrap();
    assert!(
        plain.starts_with("- HQA-1 | todo | none | Sign the lease | for human | opened ")
            && plain.matches(" | ").count() == 5,
        "a non-blocking undated item carries only its opened day: {plain}"
    );
    assert!(
        queue.contains("- HQA-5 | todo | none | Call the bank | for human | opened "),
        "{queue}"
    );
    assert!(!queue.contains("Already handled"), "{queue}");
    assert!(!queue.contains("Agent"), "{queue}");
}

#[test]
fn the_human_queue_names_people_and_is_omitted_when_empty() {
    let (m, _guard) = mcp();
    seed_project(&m, "Quiet", "QUI");
    issue(&m, "QUI", "Agent work", "todo", "high");
    let out = briefing(&m, Some("QUI"), None, &[]);
    assert!(!out.contains("Human queue"), "{out}");

    let created = m.create_issue(Parameters(CreateIssueInput {
        project: Some("QUI".into()),
        title: "Only the admin can".into(),
        assignees: Some(vec!["me".into()]),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
    let out = briefing(&m, Some("QUI"), None, &[]);
    let queue = section(&out, "Human queue (1)");
    assert!(queue.contains("Only the admin can | for @"), "{queue}");
}

#[test]
fn the_human_queue_leaves_out_projects_the_caller_cannot_view() {
    let (m, _admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    let created = as_user(&lead, || {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Shared chore".into(),
            assignees: Some(vec!["human".into()]),
            ..Default::default()
        }))
    });
    assert!(created.starts_with("Created"), "got: {created}");
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
        let hidden = queries::create_issue(
            &conn,
            &models::CreateIssue {
                project_id: foreign.id,
                title: "Shared chore".into(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO issue_assignees (issue_id, user_id) VALUES (?1, NULL)",
            [hidden.id],
        )
        .unwrap();
    }

    let out = as_user(&viewer, || briefing(&m, Some("MEM"), None, &[]));
    let queue = section(&out, "Human queue (1)");
    assert!(
        queue.contains("MEM-1 | backlog | none | Shared chore | for human | opened "),
        "{queue}"
    );
    assert!(!out.contains("FGN"), "{out}");
    assert!(!queue.contains("duplicate"), "{queue}");
}

/// Pin `updated_at` without the bump triggers undoing it (they skip a row
/// whose `seq` changes in the same statement).
fn touch_at(m: &LificMcp, identifier: &str, updated_at: &str) {
    let sequence: i64 = identifier.rsplit('-').next().unwrap().parse().unwrap();
    m.write(|conn| {
        conn.execute(
            "UPDATE issues SET updated_at = ?1, seq = seq + 1000000 WHERE sequence = ?2",
            rusqlite::params![updated_at, sequence],
        )?;
        Ok(())
    })
    .unwrap();
}

fn briefing_at(m: &LificMcp, project: &str, now: &str) -> String {
    let now = chrono::DateTime::parse_from_rfc3339(now)
        .unwrap()
        .with_timezone(&Utc);
    m.get_briefing_at(
        GetBriefingInput {
            project: Some(project.into()),
            ..Default::default()
        },
        now,
    )
    .unwrap()
}

/// LIF-492: an active issue idle past the threshold leaves the Active section
/// for its own, every active row says how long it has been idle, and the
/// threshold is the instance setting.
#[test]
fn idle_active_issues_are_reported_as_stale() {
    let (m, _guard) = mcp();
    seed_project(&m, "Stale", "STL");
    issue(&m, "STL", "Being worked", "active", "high");
    issue(&m, "STL", "Claimed and left", "active", "medium");
    issue(&m, "STL", "Old todo", "todo", "low");
    touch_at(&m, "STL-1", "2026-11-05 07:00:00");
    touch_at(&m, "STL-2", "2026-10-20 08:00:00");
    touch_at(&m, "STL-3", "2026-01-01 00:00:00");

    let out = briefing_at(&m, "STL", "2026-11-06T12:00:00Z");
    let active = section(&out, "Active (1)");
    assert!(active.contains("STL-1"), "{active}");
    assert!(active.contains("| idle 1d"), "{active}");
    assert!(!active.contains("STL-2"), "{active}");

    let stale = section(&out, "Stale active (1)");
    assert!(stale.contains("untouched for 3+ days"), "{stale}");
    assert!(stale.contains("STL-2"), "{stale}");
    assert!(stale.contains("| idle 17d"), "{stale}");
    assert!(!stale.contains("STL-3"), "a todo is never stale: {stale}");

    // A longer threshold makes the same issue current again.
    m.write(|conn| {
        queries::settings::update(
            conn,
            queries::settings::InstanceSettingsPatch {
                stale_active_days: Some(30),
                ..Default::default()
            },
        )
    })
    .unwrap();
    let out = briefing_at(&m, "STL", "2026-11-06T12:00:00Z");
    assert!(section(&out, "Active (2)").contains("STL-2"), "{out}");
    assert!(!out.contains("Stale active"), "{out}");
}
