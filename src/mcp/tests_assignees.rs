//! LIF-147: assignment through create_issue, update_issue, bulk_update and
//! list_issues, and how issue reads show it.

use rmcp::handler::server::wrapper::Parameters;

use super::*;

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

struct Setup {
    m: LificMcp,
    maintainer: models::AuthUser,
    bot: models::AuthUser,
    _guard: McpTestGuard,
}

/// The shared membership fixture (project MEM with lead, maintainer and
/// viewer; enforcement on) plus a bot owned by the maintainer.
fn setup() -> Setup {
    let (db, _admin, _lead, maintainer, _viewer, _outsider, _project_id) =
        crate::api::test_helpers::setup_membership_test();
    let bot = {
        let conn = db.write().unwrap();
        conn.execute(
            "INSERT INTO users (username, email, password_hash, is_bot, owner_id)
             VALUES ('maintainer-codex', 'codex@test.com', 'x', 1, ?1)",
            [maintainer.id],
        )
        .unwrap();
        models::AuthUser {
            id: conn.last_insert_rowid(),
            username: "maintainer-codex".into(),
            display_name: String::new(),
            is_admin: false,
        }
    };
    let au = |u: models::User| models::AuthUser {
        id: u.id,
        username: u.username,
        display_name: u.display_name,
        is_admin: u.is_admin,
    };
    Setup {
        m: LificMcp::new(db),
        maintainer: au(maintainer),
        bot,
        _guard: acquire_test_guard(),
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn create(s: &Setup, title: &str, who: Option<Vec<String>>) -> String {
    as_user(&s.maintainer, || {
        s.m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: title.into(),
            status: Some("todo".into()),
            assignees: who,
            ..Default::default()
        }))
    })
}

fn list(s: &Setup, who: &models::AuthUser, filter: &str) -> String {
    as_user(who, || {
        s.m.list_issues(Parameters(ListIssuesInput {
            project: Some("MEM".into()),
            assignee: Some(filter.into()),
            ..Default::default()
        }))
    })
}

fn get(s: &Setup, identifier: &str) -> String {
    as_user(&s.maintainer, || {
        s.m.get_issue(Parameters(GetIssueInput {
            identifier: identifier.into(),
            include_comments: Some("none".into()),
        }))
    })
}

#[test]
fn create_and_get_show_who_must_do_it() {
    let s = setup();
    assert!(create(&s, "Free", None).starts_with("Created MEM-1"));
    assert!(create(&s, "Anyone", Some(names(&["human"]))).starts_with("Created MEM-2"));
    let out = create(&s, "People", Some(names(&["lead", "@viewer"])));
    assert!(out.starts_with("Created MEM-3"), "{out}");

    assert!(!get(&s, "MEM-1").contains("Assigned:"));
    assert!(
        get(&s, "MEM-2").contains("Assigned: human (a person must do this)"),
        "{}",
        get(&s, "MEM-2")
    );
    assert!(get(&s, "MEM-3").contains("Assigned: @lead, @viewer"));

    let rows = list(&s, &s.maintainer, "human");
    assert!(
        rows.contains("MEM-2 | todo | none | Anyone for:human"),
        "{rows}"
    );
    assert!(rows.contains("for:@lead,@viewer"), "{rows}");
}

#[test]
fn a_bots_me_is_its_owner_and_bots_cannot_be_named() {
    let s = setup();
    let out = as_user(&s.bot, || {
        s.m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Check on a real device".into(),
            assignees: Some(names(&["me"])),
            ..Default::default()
        }))
    });
    assert!(out.starts_with("Created"), "{out}");
    assert!(get(&s, "MEM-1").contains("Assigned: @maintainer"));

    let out = create(&s, "Hand to the agent", Some(names(&["maintainer-codex"])));
    assert!(out.contains("only people can be assigned"), "{out}");
}

#[test]
fn update_replaces_and_clears_the_assignment() {
    let s = setup();
    create(&s, "Work", None);
    let update = |who: Option<Vec<String>>| {
        as_user(&s.maintainer, || {
            s.m.update_issue(Parameters(UpdateIssueInput {
                identifier: "MEM-1".into(),
                assignees: who,
                ..Default::default()
            }))
        })
    };
    let out = update(Some(names(&["me"])));
    assert!(out.contains("for:@maintainer"), "{out}");
    let out = update(Some(names(&["human"])));
    assert!(out.contains("for:human"), "{out}");
    let out = update(Some(names(&[])));
    assert!(!out.contains("for:"), "{out}");
    let out = update(Some(names(&["human", "lead"])));
    assert!(out.contains("not both"), "{out}");
    let out = update(Some(names(&["non_member"])));
    assert!(out.contains("not a member of this project"), "{out}");
}

#[test]
fn list_filters_by_assignment() {
    let s = setup();
    create(&s, "Free", None);
    create(&s, "Anyone", Some(names(&["human"])));
    create(&s, "Mine", Some(names(&["me"])));

    let free = list(&s, &s.maintainer, "none");
    assert!(free.contains("Free") && !free.contains("Anyone") && !free.contains("Mine"));
    let mine = list(&s, &s.maintainer, "me");
    assert!(mine.contains("Mine") && !mine.contains("Anyone"), "{mine}");
    // An agent asking for "me" gets its owner's issues.
    let owners = list(&s, &s.bot, "me");
    assert!(owners.contains("Mine"), "{owners}");
    let people = list(&s, &s.maintainer, "human");
    assert!(people.contains("Anyone") && people.contains("Mine") && !people.contains("Free"));
    let err = list(&s, &s.maintainer, "ghost");
    assert!(err.contains("no user named 'ghost'"), "{err}");
}

#[test]
fn bulk_update_sets_the_assignment_on_every_match() {
    let s = setup();
    create(&s, "One", None);
    create(&s, "Two", None);
    let out = as_user(&s.maintainer, || {
        s.m.bulk_update(Parameters(BulkUpdateInput {
            project: "MEM".into(),
            filter_status: Some("todo".into()),
            set_assignees: Some(names(&["human"])),
            ..Default::default()
        }))
    });
    assert_eq!(out, "Updated 2 issue(s)");
    assert!(list(&s, &s.maintainer, "none").contains("No issues found."));
}

#[test]
fn a_batch_create_resolves_me_per_item() {
    let s = setup();
    let out = as_user(&s.maintainer, || {
        s.m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            issues: Some(vec![
                CreateIssueItem {
                    title: "Mine".into(),
                    assignees: Some(names(&["me"])),
                    ..Default::default()
                },
                CreateIssueItem {
                    title: "Agents'".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }))
    });
    assert!(out.starts_with("Created 2 issues"), "{out}");
    assert!(get(&s, "MEM-1").contains("Assigned: @maintainer"));
    assert!(!get(&s, "MEM-2").contains("Assigned:"));
}

#[test]
fn activity_names_assignment_changes() {
    let s = setup();
    create(&s, "Work", Some(names(&["lead"])));
    let out = as_user(&s.maintainer, || {
        s.m.get_activity(Parameters(GetActivityInput {
            identifier: "MEM-1".into(),
            ..Default::default()
        }))
    });
    assert!(out.contains("+assignee @lead"), "{out}");
}
