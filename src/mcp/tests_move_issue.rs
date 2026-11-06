//! LIF-487: `move_issue`, and how `get_issue` reads a moved issue by its old
//! identifier.

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
    target: i64,
    events: tokio::sync::broadcast::Receiver<crate::realtime::RealtimeMessage>,
    _guard: McpTestGuard,
}

/// The shared membership fixture (project MEM, enforcement on) plus project
/// DST where the maintainer starts as a Viewer, and one labelled issue MEM-1
/// in module "Core".
fn setup() -> Setup {
    let (db, _admin, _lead, maintainer, _viewer, _outsider, source) =
        crate::api::test_helpers::setup_membership_test();
    let target = {
        let conn = db.write().unwrap();
        let target = queries::create_project(
            &conn,
            &models::CreateProject {
                name: "Destination".into(),
                identifier: "DST".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
        queries::members::upsert_member(&conn, target, maintainer.id, models::Role::Viewer)
            .unwrap();
        queries::create_label(
            &conn,
            &models::CreateLabel {
                project_id: source,
                name: "bug".into(),
                color: "#ff0000".into(),
            },
        )
        .unwrap();
        let core = queries::create_module(
            &conn,
            &models::CreateModule {
                project_id: source,
                name: "Core".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap()
        .id;
        queries::create_issue(
            &conn,
            &models::CreateIssue {
                project_id: source,
                title: "Wandering".into(),
                module_id: Some(core),
                labels: vec!["bug".into()],
                ..Default::default()
            },
        )
        .unwrap();
        target
    };
    let realtime = crate::realtime::RealtimeHub::new();
    let events = realtime.subscribe();
    Setup {
        m: LificMcp::with_realtime(db, realtime),
        maintainer: models::AuthUser {
            id: maintainer.id,
            username: maintainer.username,
            display_name: maintainer.display_name,
            is_admin: false,
        },
        target,
        events,
        _guard: acquire_test_guard(),
    }
}

fn move_input(project: &str, module: Option<&str>) -> MoveIssueInput {
    MoveIssueInput {
        identifier: "MEM-1".into(),
        project: project.into(),
        module: module.map(Into::into),
    }
}

#[test]
fn a_moved_issue_is_found_by_its_old_identifier() {
    let mut s = setup();
    let m = s.m.clone();
    let out = as_user(&s.maintainer, || {
        m.move_issue(Parameters(move_input("DST", None)))
    });
    // Refused until the caller can write to the target too.
    assert!(
        out.contains("Forbidden"),
        "viewer on DST must be refused: {out}"
    );
    {
        let conn = s.m.db.write().unwrap();
        queries::members::upsert_member(&conn, s.target, s.maintainer.id, models::Role::Maintainer)
            .unwrap();
    }
    while s.events.try_recv().is_ok() {}
    let out = as_user(&s.maintainer, || {
        m.move_issue(Parameters(move_input("DST", None)))
    });
    assert!(out.starts_with("Moved MEM-1 to "), "{out}");
    assert!(out.contains("DST-1"), "{out}");
    assert!(out.contains("MEM-1 still resolves"), "{out}");
    assert!(
        out.contains("Dropped labels (no label of that name here): bug"),
        "{out}"
    );
    assert!(out.contains("Cleared module Core"), "{out}");

    let events: Vec<_> = std::iter::from_fn(|| s.events.try_recv().ok())
        .map(|message| message.event)
        .collect();
    assert!(matches!(
        events.as_slice(),
        [
            crate::realtime::RealtimeEvent::IssueDeleted { .. },
            crate::realtime::RealtimeEvent::IssueCreated { project_id, .. },
        ] if *project_id == s.target
    ));

    let read = as_user(&s.maintainer, || {
        m.get_issue(Parameters(GetIssueInput {
            identifier: "MEM-1".into(),
            ..Default::default()
        }))
    });
    assert!(read.contains("DST-1"), "{read}");
    assert!(read.contains("Wandering"), "{read}");
    assert!(read.contains("Formerly: MEM-1"), "{read}");
}

#[test]
fn a_module_must_exist_in_the_target_project() {
    let s = setup();
    {
        let conn = s.m.db.write().unwrap();
        queries::members::upsert_member(&conn, s.target, s.maintainer.id, models::Role::Maintainer)
            .unwrap();
    }
    let m = s.m.clone();
    // "Core" lives in MEM, not DST.
    let out = as_user(&s.maintainer, || {
        m.move_issue(Parameters(move_input("DST", Some("Core"))))
    });
    assert!(out.starts_with("Error"), "{out}");
    let read = as_user(&s.maintainer, || {
        m.get_issue(Parameters(GetIssueInput {
            identifier: "MEM-1".into(),
            ..Default::default()
        }))
    });
    assert!(!read.contains("Formerly"), "{read}");
}
