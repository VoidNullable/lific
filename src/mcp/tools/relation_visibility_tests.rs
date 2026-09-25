//! LIF-488: MCP reads never name a related issue in a project the caller
//! cannot view. Export has the same guard (see the export tests).

use super::*;
use rmcp::handler::server::wrapper::Parameters;

/// In the membership project: the blocker blocks a visible issue and a hidden
/// one (SEC), and the hidden issue also blocks the visible one, so both the
/// `blocks` and `blocked_by` directions carry a hidden identifier.
fn seed(m: &LificMcp, project_id: i64) -> [String; 3] {
    let conn = m.db.write().unwrap();
    let [blocker, visible, hidden] = crate::export::seed_hidden_relation(&conn, project_id);
    let id = |identifier: &str| queries::resolve_identifier(&conn, identifier).unwrap();
    queries::link_issues(&conn, id(&hidden), id(&visible), "blocks").unwrap();
    [blocker, visible, hidden]
}

/// Every MCP read that renders relations, as `user`.
async fn reads_as(
    m: &LificMcp,
    user: &models::AuthUser,
    blocker: &str,
    visible: &str,
) -> Vec<(&'static str, String)> {
    let m = m.clone();
    let (blocker, visible) = (blocker.to_owned(), visible.to_owned());
    crate::mcp::with_request_user(Some(user.clone()), || async move {
        vec![
            (
                "get_issue blocker",
                m.get_issue(Parameters(GetIssueInput {
                    identifier: blocker.clone(),
                    ..Default::default()
                })),
            ),
            (
                "get_issue visible",
                m.get_issue(Parameters(GetIssueInput {
                    identifier: visible.clone(),
                    ..Default::default()
                })),
            ),
            (
                "list_issues",
                m.list_issues(Parameters(ListIssuesInput {
                    project: Some("MEM".into()),
                    ..Default::default()
                })),
            ),
            (
                "get_board",
                m.get_board(Parameters(GetBoardInput {
                    project: Some("MEM".into()),
                    ..Default::default()
                })),
            ),
        ]
    })
    .await
}

#[tokio::test]
async fn issue_reads_leave_out_relations_to_projects_the_caller_cannot_view() {
    let (m, admin, _, _, viewer, _, project_id, _guard) = super::tests::setup_membership_mcp();
    let [blocker, visible, hidden] = seed(&m, project_id);

    for (surface, output) in reads_as(&m, &viewer, &blocker, &visible).await {
        assert!(!output.starts_with("Error"), "{surface}: {output}");
        assert!(
            !output.contains(&hidden),
            "{surface} leaked {hidden}: {output}"
        );
    }
    let scoped_blocker = &reads_as(&m, &viewer, &blocker, &visible).await[0].1;
    assert!(
        scoped_blocker.contains(&visible),
        "visible relation kept: {scoped_blocker}"
    );

    let full = reads_as(&m, &admin, &blocker, &visible).await;
    assert!(
        full[0].1.contains(&hidden),
        "admin sees blocks: {}",
        full[0].1
    );
    assert!(
        full[1].1.contains(&hidden),
        "admin sees blocked_by: {}",
        full[1].1
    );
}

#[tokio::test]
async fn write_echoes_leave_out_relations_to_projects_the_caller_cannot_view() {
    let (m, _, _, maintainer, _, _, project_id, _guard) = super::tests::setup_membership_mcp();
    let [blocker, _, hidden] = seed(&m, project_id);
    let m2 = m.clone();
    let blocker2 = blocker.clone();
    let (updated, edited) =
        crate::mcp::with_request_user(Some(maintainer.clone()), || async move {
            let updated = m2.update_issue(Parameters(UpdateIssueInput {
                identifier: blocker2.clone(),
                priority: Some("high".into()),
                ..Default::default()
            }));
            let edited = m2.edit_issue(Parameters(EditIssueInput {
                identifier: blocker2.clone(),
                field: Some("title".into()),
                old_string: "Blocker".into(),
                new_string: "Blocker (edited)".into(),
                ..Default::default()
            }));
            (updated, edited)
        })
        .await;
    for (surface, output) in [("update_issue", updated), ("edit_issue", edited)] {
        assert!(
            output.starts_with("Updated") || output.starts_with("Edited"),
            "{surface}: {output}"
        );
        assert!(
            !output.contains(&hidden),
            "{surface} leaked {hidden}: {output}"
        );
    }
}
