//! GitHub #87: project roles, members filter and status counts over MCP.
//!
//! Tools are called with the JSON arguments a client sends, so these tests
//! double as examples of the wire format.

use super::tests::setup_membership_mcp;
use super::*;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::{Value, json};

/// Run `f` as `user` (`None`: a credential without a user) through the
/// production MCP identity wrapper.
fn as_user(user: Option<&models::AuthUser>, f: impl FnOnce() -> String) -> String {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_user(user.cloned(), || async {
            f()
        }))
}

fn tool_input<T: serde::de::DeserializeOwned>(args: Value) -> T {
    serde_json::from_value(args).expect("valid tool arguments")
}

/// `list_resources(resource_type="project", ..args)` as `user`.
fn projects(m: &LificMcp, user: Option<&models::AuthUser>, mut args: Value) -> String {
    args["resource_type"] = json!("project");
    as_user(user, || m.list_resources(Parameters(tool_input(args))))
}

/// The row for `identifier` in a project listing.
fn row<'a>(listing: &'a str, identifier: &str) -> &'a str {
    let prefix = format!("- {identifier} |");
    listing
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no row for {identifier} in: {listing}"))
}

#[track_caller]
fn assert_ends(text: &str, suffix: &str) {
    assert!(
        text.ends_with(suffix),
        "expected ...{suffix:?}\n got: {text}"
    );
}

#[track_caller]
fn assert_has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected {needle:?}\n got: {text}");
}

/// Create an active user and give them `role` on `project_id`.
fn add_member(m: &LificMcp, project_id: i64, username: &str, role: models::Role) {
    m.write(|conn| {
        let user = queries::users::create_user(
            conn,
            &models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )?;
        queries::members::upsert_member(conn, project_id, user.id, role)
    })
    .unwrap();
}

/// Insert issues with the given statuses straight into `project_id`.
fn seed_statuses(m: &LificMcp, project_id: i64, statuses: &[&str]) {
    m.write(|conn| {
        for status in statuses {
            conn.execute(
                "INSERT INTO issues (project_id, sequence, title, status)
                 VALUES (?1, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM issues
                              WHERE project_id = ?1), 'Seeded', ?2)",
                rusqlite::params![project_id, status],
            )?;
        }
        Ok(())
    })
    .unwrap();
}

/// A second project, `OTH`, led by `lead_id`.
fn seed_other_project(m: &LificMcp, lead_id: i64) -> i64 {
    m.write(|conn| {
        Ok(queries::create_project(
            conn,
            &models::CreateProject {
                name: "Other".into(),
                identifier: "OTH".into(),
                lead_user_id: Some(lead_id),
                ..Default::default()
            },
        )?
        .id)
    })
    .unwrap()
}

// ── Project rows ──────────────────────────────────────────────────────

#[test]
fn project_row_names_leads_and_maintainers_counts_viewers_and_shows_your_role() {
    let (m, admin, _lead, _maintainer, viewer, .., _guard) = setup_membership_mcp();

    let as_viewer = projects(&m, Some(&viewer), json!({}));
    assert_ends(
        row(&as_viewer, "MEM"),
        " | lead @lead · maintainer @maintainer · 1 viewer | you: viewer | no issues",
    );

    let as_admin = projects(&m, Some(&admin), json!({}));
    assert_ends(row(&as_admin, "MEM"), " | you: admin | no issues");
}

#[test]
fn a_single_person_instance_hides_the_roster_and_owns_every_project() {
    let (m, _guard) = super::tests::mcp();
    super::tests::seed_project(&m, "Solo", "SOL");
    let pid = super::tests::project_id_for(&m, "SOL");
    seed_statuses(&m, pid, &["todo"]);
    let my_issues = || {
        as_user(None, || {
            m.list_issues(Parameters(tool_input(json!({"members": ["me"]}))))
        })
    };

    let solo = projects(&m, None, json!({}));
    assert_ends(row(&solo, "SOL"), " ago) | 1 todo");
    // A project created without a lead still belongs to the only person.
    assert_ends(
        row(&projects(&m, None, json!({"show_members": []})), "SOL"),
        " | 1 lead | you: lead | 1 todo",
    );
    let mine = my_issues();
    assert!(mine.starts_with("Your roles: SOL lead\n"), "got: {mine}");
    assert_has(&mine, "SOL-1");

    add_member(&m, pid, "partner", models::Role::Viewer);
    assert_ends(
        row(&projects(&m, None, json!({})), "SOL"),
        " | 1 viewer | you: admin | 1 todo",
    );
    assert_eq!(
        my_issues(),
        "No projects where you have a role (among projects you can see)."
    );
}

#[test]
fn roster_names_five_per_role_and_show_members_lists_chosen_roles_in_full() {
    let (m, _admin, lead, _maintainer, _viewer, _non_member, pid, _guard) = setup_membership_mcp();
    for name in ["m2", "m3", "m4", "m5", "m6", "m7"] {
        add_member(&m, pid, name, models::Role::Maintainer);
    }
    add_member(&m, pid, "v2", models::Role::Viewer);
    let roster = |show: Value| {
        let out = projects(
            &m,
            Some(&lead),
            json!({"show_members": show, "statuses": []}),
        );
        row(&out, "MEM").to_owned()
    };

    assert_ends(
        &roster(Value::Null),
        " | lead @lead · maintainers @maintainer, @m2, @m3, @m4, @m5 +2 more · 2 viewers | you: lead",
    );
    assert_has(
        &roster(json!(["all"])),
        "maintainers @maintainer, @m2, @m3, @m4, @m5, @m6, @m7 · viewers @viewer, @v2",
    );
    assert_has(
        &roster(json!(["viewer"])),
        "@m5 +2 more · viewers @viewer, @v2",
    );
    assert_ends(
        &roster(json!([])),
        " | 1 lead · 7 maintainers · 2 viewers | you: lead",
    );
}

#[test]
fn deactivated_users_are_left_off_the_roster() {
    let (m, _admin, lead, _maintainer, _viewer, _non_member, pid, _guard) = setup_membership_mcp();
    add_member(&m, pid, "gone", models::Role::Maintainer);
    m.write(|conn| Ok(conn.execute("UPDATE users SET is_active = 0 WHERE username = 'gone'", [])?))
        .unwrap();

    let out = projects(&m, Some(&lead), json!({"show_members": ["all"]}));

    assert!(!out.contains("@gone"), "got: {out}");
}

#[test]
fn project_row_counts_issues_per_status_and_omits_zeros() {
    let (m, _admin, lead, _maintainer, _viewer, _non_member, pid, _guard) = setup_membership_mcp();
    seed_statuses(&m, pid, &["todo", "active", "todo", "backlog", "done"]);

    let out = projects(&m, Some(&lead), json!({}));

    assert_ends(
        row(&out, "MEM"),
        " | you: lead | 1 active · 2 todo · 1 backlog · 1 done",
    );
}

#[test]
fn statuses_picks_which_counts_a_project_row_prints() {
    let (m, _admin, lead, _maintainer, _viewer, _non_member, pid, _guard) = setup_membership_mcp();
    seed_statuses(&m, pid, &["todo", "active", "backlog", "done", "done"]);
    let counts = |statuses: Value| {
        let out = projects(&m, Some(&lead), json!({"statuses": statuses}));
        row(&out, "MEM").to_owned()
    };

    assert_ends(
        &counts(json!(["Todo", " active "])),
        " | you: lead | 1 active · 1 todo",
    );
    assert_ends(
        &counts(json!(["all"])),
        " | 1 active · 1 todo · 1 backlog · 2 done",
    );
    assert_ends(&counts(json!(["cancelled"])), " | you: lead | 0 cancelled");
    assert_ends(&counts(json!([])), " | you: lead");

    let bad = projects(&m, Some(&lead), json!({"statuses": ["todo", "doing"]}));
    assert!(bad.starts_with("Error"), "got: {bad}");
    assert_has(&bad, "'doing'");
}

#[test]
fn project_narrows_the_listing_behind_the_viewer_gate() {
    let (m, admin, _lead, maintainer, .., _guard) = setup_membership_mcp();
    seed_other_project(&m, admin.id);

    let one = projects(&m, Some(&admin), json!({"project": "oth"}));
    assert!(one.starts_with("1 projects:\n- OTH |"), "got: {one}");

    let hidden = projects(&m, Some(&maintainer), json!({"project": "OTH"}));
    assert!(hidden.starts_with("Error: Forbidden"), "got: {hidden}");
}

// ── members / roles ───────────────────────────────────────────────────

#[test]
fn members_me_keeps_only_projects_where_an_admin_holds_a_role() {
    let (m, admin, .., _guard) = setup_membership_mcp();
    seed_other_project(&m, admin.id);

    let everything = projects(&m, Some(&admin), json!({}));
    assert!(everything.starts_with("2 projects:"), "got: {everything}");

    let mine = projects(&m, Some(&admin), json!({"members": ["me"]}));
    assert!(mine.starts_with("1 projects:"), "got: {mine}");
    assert_has(row(&mine, "OTH"), "you: lead");
}

#[test]
fn members_by_username_and_several_names_union() {
    let (m, admin, .., _guard) = setup_membership_mcp();
    seed_other_project(&m, admin.id);

    let leads = projects(&m, Some(&admin), json!({"members": ["@Lead"]}));
    assert!(leads.starts_with("1 projects:\n- MEM |"), "got: {leads}");

    let both = projects(&m, Some(&admin), json!({"members": ["lead", "me"]}));
    assert!(both.starts_with("2 projects:"), "got: {both}");
}

#[test]
fn members_never_reveals_a_project_the_caller_cannot_see() {
    let (m, admin, _lead, maintainer, .., _guard) = setup_membership_mcp();
    seed_other_project(&m, admin.id);

    let out = projects(&m, Some(&maintainer), json!({"members": ["admin"]}));

    assert_eq!(
        out,
        "No projects where @admin has a role (among projects you can see)."
    );
}

#[test]
fn roles_narrow_members_and_alone_mean_your_own_roles() {
    let (m, _admin, lead, maintainer, .., _guard) = setup_membership_mcp();

    let lead_elsewhere = projects(
        &m,
        Some(&maintainer),
        json!({"members": ["lead"], "roles": ["maintainer", "viewer"]}),
    );
    assert!(
        lead_elsewhere.starts_with("No projects where @lead has a role"),
        "got: {lead_elsewhere}"
    );

    let own = projects(&m, Some(&lead), json!({"roles": ["LEAD"]}));
    row(&own, "MEM");

    let own_viewer = projects(&m, Some(&lead), json!({"roles": ["viewer"]}));
    assert!(
        own_viewer.starts_with("No projects where you have a role"),
        "got: {own_viewer}"
    );
}

#[test]
fn members_rejects_unknown_users_empty_lists_all_and_bad_roles() {
    let (m, _admin, lead, .., _guard) = setup_membership_mcp();
    let call = |args: Value| projects(&m, Some(&lead), args);

    assert_has(
        &call(json!({"members": ["ghost"]})),
        "no active user named 'ghost'",
    );
    assert_has(
        &call(json!({"members": []})),
        "members must name at least one user",
    );
    assert_has(
        &call(json!({"members": ["all"]})),
        "'all' is not allowed in members",
    );
    assert_has(
        &call(json!({"members": ["me"], "roles": ["owner"]})),
        "invalid role 'owner'",
    );
}

#[test]
fn me_is_the_callers_effective_user() {
    let (m, admin, lead, .., _guard) = setup_membership_mcp();
    seed_other_project(&m, admin.id);
    let bot = m
        .write(|conn| queries::users::create_bot_user(conn, lead.id, "lead-bot", "Lead bot", None))
        .unwrap();
    let bot = models::AuthUser {
        id: bot.id,
        username: bot.username,
        display_name: bot.display_name,
        is_admin: false,
    };

    // A bot answers as its owner.
    let as_bot = projects(&m, Some(&bot), json!({"members": ["me"]}));
    assert!(as_bot.starts_with("1 projects:\n- MEM |"), "got: {as_bot}");
    assert_has(&as_bot, "you: lead");

    // A credential without a user answers as the first admin.
    let unbound = projects(&m, None, json!({"members": ["me"]}));
    assert!(
        unbound.starts_with("1 projects:\n- OTH |"),
        "got: {unbound}"
    );
}

#[test]
fn legacy_mode_non_member_sees_rosters_but_holds_no_role() {
    let (m, _admin, _lead, _maintainer, _viewer, non_member, .., _guard) = setup_membership_mcp();
    m.write(|conn| {
        queries::settings::update(
            conn,
            queries::settings::InstanceSettingsPatch {
                authz_enforced: Some(false),
                ..Default::default()
            },
        )
        .map(|_| ())
    })
    .unwrap();

    let listing = projects(&m, Some(&non_member), json!({}));
    assert_ends(
        row(&listing, "MEM"),
        " | lead @lead · maintainer @maintainer · 1 viewer | no issues",
    );

    let mine = projects(&m, Some(&non_member), json!({"members": ["me"]}));
    assert_eq!(
        mine,
        "No projects where you have a role (among projects you can see)."
    );
}

// ── Schema ────────────────────────────────────────────────────────────

/// Assert `tool` advertises `fields` as optional, described string lists.
#[track_caller]
fn assert_optional_string_lists(m: &LificMcp, tool: &str, fields: &[&str]) {
    let (_, schema) = m
        .list_tool_schemas()
        .into_iter()
        .find(|(name, _)| name == tool)
        .unwrap_or_else(|| panic!("no tool {tool}"));
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    for field in fields {
        let property = &schema["properties"][field];
        assert_eq!(
            property["items"]["type"], "string",
            "{tool}.{field}: {property}"
        );
        assert!(
            property["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "{tool}.{field}"
        );
        assert!(
            !required.contains(&json!(field)),
            "{tool}.{field} must stay optional"
        );
    }
}

#[test]
fn list_resources_advertises_the_new_filters_as_optional_string_lists() {
    let (m, .., _guard) = setup_membership_mcp();
    assert_optional_string_lists(
        &m,
        "list_resources",
        &["members", "roles", "statuses", "show_members"],
    );
}

// ── list_issues across a member's projects ────────────────────────────

/// Insert an issue straight into `project_id`; `age` backdates `updated_at`
/// on insert (the `issues_updated` trigger restamps any later UPDATE).
fn seed_issue_as(
    m: &LificMcp,
    project_id: i64,
    title: &str,
    status: &str,
    priority: &str,
    age: &str,
) {
    m.write(|conn| {
        conn.execute(
            "INSERT INTO issues (project_id, sequence, title, status, priority, updated_at)
             VALUES (?1, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM issues WHERE project_id = ?1),
                     ?2, ?3, ?4, datetime('now', ?5))",
            rusqlite::params![project_id, title, status, priority, age],
        )?;
        Ok(())
    })
    .unwrap();
}

/// MEM (lead: `lead`, maintainer: `maintainer`) and OTH (lead: `admin`,
/// viewer: `lead`), with open and closed work in both.
fn triage_fixture() -> (
    LificMcp,
    models::AuthUser,
    models::AuthUser,
    i64,
    McpTestGuard,
) {
    let (m, admin, lead, maintainer, _viewer, _non_member, mem, guard) = setup_membership_mcp();
    let oth = seed_other_project(&m, admin.id);
    m.write(|conn| queries::members::upsert_member(conn, oth, lead.id, models::Role::Viewer))
        .unwrap();
    seed_issue_as(&m, mem, "Low todo", "todo", "low", "+0 days"); // MEM-1
    seed_issue_as(&m, mem, "Plain active", "active", "none", "+0 days"); // MEM-2
    seed_issue_as(&m, mem, "Finished", "done", "high", "+0 days"); // MEM-3
    seed_issue_as(&m, oth, "Urgent todo", "todo", "urgent", "+0 days"); // OTH-1
    seed_issue_as(&m, oth, "Dropped", "cancelled", "urgent", "+0 days"); // OTH-2
    (m, lead, maintainer, mem, guard)
}

fn issues(m: &LificMcp, user: &models::AuthUser, args: Value) -> String {
    as_user(Some(user), || m.list_issues(Parameters(tool_input(args))))
}

/// Issue identifiers in the order a listing prints them.
fn listed(out: &str) -> Vec<&str> {
    out.lines()
        .filter_map(|line| line.strip_prefix("- ")?.split(' ').next())
        .collect()
}

#[test]
fn my_issues_match_the_home_page_active_then_todo_then_priority() {
    let (m, lead, ..) = triage_fixture();

    let out = issues(&m, &lead, json!({"members": ["me"]}));

    assert!(
        out.starts_with("Your roles: MEM lead · OTH viewer\n3 issues:\n"),
        "got: {out}"
    );
    assert_eq!(listed(&out), ["MEM-2", "OTH-1", "MEM-1"], "got: {out}");
}

#[test]
fn triage_breaks_ties_by_recency_and_backlog_comes_on_request() {
    let (m, lead, _maintainer, mem, _guard) = triage_fixture();
    seed_issue_as(&m, mem, "Older todo", "todo", "none", "-2 days"); // MEM-4
    seed_issue_as(&m, mem, "Newer todo", "todo", "none", "+0 days"); // MEM-5
    seed_issue_as(&m, mem, "Plain backlog", "backlog", "none", "+0 days"); // MEM-6

    let out = issues(
        &m,
        &lead,
        json!({"members": ["me"], "priority": "none", "statuses": ["active", "todo", "backlog"]}),
    );

    assert_eq!(
        listed(&out),
        ["MEM-2", "MEM-5", "MEM-4", "MEM-6"],
        "got: {out}"
    );
}

#[test]
fn statuses_brings_closed_work_back_and_explicit_order_by_wins() {
    let (m, lead, ..) = triage_fixture();

    // Lowest priority first: an order triage would never produce.
    let all = issues(
        &m,
        &lead,
        json!({"members": ["me"], "statuses": ["all"], "order_by": "priority", "order": "desc"}),
    );
    assert_eq!(
        listed(&all),
        ["MEM-2", "MEM-1", "MEM-3", "OTH-2", "OTH-1"],
        "got: {all}"
    );

    let closed = issues(
        &m,
        &lead,
        json!({"members": ["me"], "statuses": ["done", "Cancelled"]}),
    );
    assert_eq!(listed(&closed), ["OTH-2", "MEM-3"], "got: {closed}");

    let one_project = issues(
        &m,
        &lead,
        json!({"project": "MEM", "statuses": ["todo", "done"]}),
    );
    assert!(
        one_project.starts_with("2 issues:"),
        "no header without members: {one_project}"
    );
    assert_eq!(listed(&one_project), ["MEM-1", "MEM-3"]);
}

#[test]
fn roles_alone_lists_my_issues_where_i_hold_those_roles() {
    let (m, lead, ..) = triage_fixture();

    let out = issues(&m, &lead, json!({"roles": ["lead"]}));

    assert!(out.starts_with("Your roles: MEM lead\n"), "got: {out}");
    assert_eq!(listed(&out), ["MEM-2", "MEM-1"], "got: {out}");
}

#[test]
fn another_members_issues_stay_limited_to_projects_i_can_see() {
    let (m, _lead, maintainer, ..) = triage_fixture();

    let out = issues(&m, &maintainer, json!({"members": ["@admin", "lead"]}));

    assert!(
        out.starts_with("Roles: MEM @lead lead\n"),
        "OTH is hidden from a MEM maintainer: {out}"
    );
    assert_eq!(listed(&out), ["MEM-2", "MEM-1"], "got: {out}");
}

#[test]
fn members_with_project_filters_that_project() {
    let (m, lead, maintainer, ..) = triage_fixture();

    let in_mem = issues(
        &m,
        &lead,
        json!({"project": "MEM", "members": ["maintainer"]}),
    );
    assert_eq!(listed(&in_mem), ["MEM-2", "MEM-1"], "got: {in_mem}");

    let no_role = issues(
        &m,
        &lead,
        json!({"project": "OTH", "members": ["maintainer"]}),
    );
    assert_eq!(no_role, "No issues: @maintainer has no role in OTH.");

    let hidden = issues(
        &m,
        &maintainer,
        json!({"project": "OTH", "members": ["me"]}),
    );
    assert!(hidden.starts_with("Error: Forbidden"), "got: {hidden}");
}

#[test]
fn an_empty_member_listing_still_names_the_roles() {
    let (m, lead, ..) = triage_fixture();

    let out = issues(&m, &lead, json!({"members": ["me"], "priority": "medium"}));

    assert_eq!(out, "Your roles: MEM lead · OTH viewer\nNo issues found.");
}

#[test]
fn list_issues_rejects_ambiguous_filters_and_keeps_project_required_otherwise() {
    let (m, lead, ..) = triage_fixture();
    let call = |args: Value| issues(&m, &lead, args);

    let cases = [
        (
            json!({"project": "MEM", "status": "todo", "statuses": ["done"]}),
            "pass status or statuses, not both",
        ),
        (
            json!({"project": "MEM", "statuses": []}),
            "statuses must name at least one status",
        ),
        (
            json!({"members": ["me"], "module": "Core"}),
            "module requires project",
        ),
        (
            json!({"members": ["me"], "label": "bug"}),
            "label requires project",
        ),
        (
            json!({"members": ["me"], "order": "desc"}),
            "order needs order_by",
        ),
        (json!({}), "project required"),
    ];
    for (args, error) in cases {
        let out = call(args.clone());
        assert!(out.starts_with("Error"), "{args}: {out}");
        assert_has(&out, error);
    }
}

#[test]
fn list_issues_advertises_members_roles_and_statuses_as_optional_string_lists() {
    let (m, ..) = setup_membership_mcp();
    assert_optional_string_lists(&m, "list_issues", &["members", "roles", "statuses"]);
}

#[test]
fn quoted_values_are_accepted_like_other_mcp_arguments() {
    let (m, lead, ..) = triage_fixture();

    let out = issues(
        &m,
        &lead,
        json!({"members": ["'lead'"], "statuses": ["\"todo\""]}),
    );

    assert_eq!(listed(&out), ["OTH-1", "MEM-1"], "got: {out}");
}
