//! LIF-518: who lost project access when legacy authorization was retired.
//!
//! Migration 070 seeded memberships from evidence and recorded every active
//! user left without a role in a project they could reach before, in
//! `authz_upgrade_losses`. This module reads that record for the startup log
//! and `lific doctor`. A row stops counting once the user holds any role in
//! the project again, becomes an admin, or is deactivated; deleting the user
//! or the project removes it outright.

use rusqlite::Connection;

use crate::error::LificError;

/// One user who could reach a project before 3.0 and holds no role in it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLoss {
    pub username: String,
    pub project: String,
}

const UNRESOLVED: &str = "
    FROM authz_upgrade_losses l
    JOIN users u ON u.id = l.user_id
    JOIN projects p ON p.id = l.project_id
    WHERE u.is_admin = 0
      AND u.is_active = 1
      AND NOT EXISTS (
          SELECT 1 FROM project_members pm
          WHERE pm.project_id = l.project_id AND pm.user_id = l.user_id
      )";

/// Every loss not yet restored, ordered by user then project.
pub fn unresolved(conn: &Connection) -> Result<Vec<AccessLoss>, LificError> {
    let sql =
        format!("SELECT u.username, p.identifier {UNRESOLVED} ORDER BY u.username, p.identifier");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(AccessLoss {
            username: row.get(0)?,
            project: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The unresolved losses no server start has logged yet, marking them logged.
/// Used once per upgrade so the startup log names them a single time.
pub fn take_unlogged(conn: &Connection) -> Result<Vec<AccessLoss>, LificError> {
    let sql = format!(
        "SELECT u.username, p.identifier {UNRESOLVED} AND l.logged_at IS NULL
         ORDER BY u.username, p.identifier"
    );
    let losses = {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(AccessLoss {
                username: row.get(0)?,
                project: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    conn.execute(
        "UPDATE authz_upgrade_losses SET logged_at = datetime('now') WHERE logged_at IS NULL",
        [],
    )?;
    Ok(losses)
}

/// One line per user, `alice: API, WEB`, for logs and `lific doctor`.
pub fn summarize(losses: &[AccessLoss]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    for loss in losses {
        match &mut current {
            Some((user, projects)) if *user == loss.username => projects.push(&loss.project),
            _ => {
                if let Some((user, projects)) = current.take() {
                    lines.push(format!("{user}: {}", projects.join(", ")));
                }
                current = Some((&loss.username, vec![&loss.project]));
            }
        }
    }
    if let Some((user, projects)) = current {
        lines.push(format!("{user}: {}", projects.join(", ")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::{CreateProject, CreateUser, Role};
    use crate::db::queries;

    fn user(conn: &Connection, name: &str) -> i64 {
        queries::users::create_user(
            conn,
            &CreateUser {
                username: name.into(),
                email: format!("{name}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    fn project(conn: &Connection, ident: &str) -> i64 {
        queries::create_project(
            conn,
            &CreateProject {
                name: ident.into(),
                identifier: ident.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn record(conn: &Connection, user_id: i64, project_id: i64) {
        conn.execute(
            "INSERT INTO authz_upgrade_losses (user_id, project_id) VALUES (?1, ?2)",
            [user_id, project_id],
        )
        .unwrap();
    }

    #[test]
    fn a_restored_role_clears_the_loss() {
        let pool = db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        let bob = user(&conn, "bob");
        let api = project(&conn, "API");
        let web = project(&conn, "WEB");
        record(&conn, bob, api);
        record(&conn, bob, web);

        assert_eq!(unresolved(&conn).unwrap().len(), 2);
        queries::members::upsert_member(&conn, web, bob, Role::Viewer).unwrap();
        assert_eq!(
            unresolved(&conn).unwrap(),
            vec![AccessLoss {
                username: "bob".into(),
                project: "API".into()
            }]
        );
    }

    #[test]
    fn deactivated_and_promoted_users_are_not_reported() {
        let pool = db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        let gone = user(&conn, "gone");
        let boss = user(&conn, "boss");
        let api = project(&conn, "API");
        record(&conn, gone, api);
        record(&conn, boss, api);
        conn.execute("UPDATE users SET is_active = 0 WHERE id = ?1", [gone])
            .unwrap();
        conn.execute("UPDATE users SET is_admin = 1 WHERE id = ?1", [boss])
            .unwrap();

        assert!(unresolved(&conn).unwrap().is_empty());
    }

    #[test]
    fn take_unlogged_returns_each_loss_once() {
        let pool = db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        let bob = user(&conn, "bob");
        let api = project(&conn, "API");
        record(&conn, bob, api);

        assert_eq!(take_unlogged(&conn).unwrap().len(), 1);
        assert!(take_unlogged(&conn).unwrap().is_empty());
        assert_eq!(unresolved(&conn).unwrap().len(), 1, "still unresolved");
    }

    #[test]
    fn summarize_groups_projects_by_user() {
        let loss = |u: &str, p: &str| AccessLoss {
            username: u.into(),
            project: p.into(),
        };
        assert_eq!(
            summarize(&[loss("amy", "API"), loss("amy", "WEB"), loss("bob", "API")]),
            vec!["amy: API, WEB".to_string(), "bob: API".to_string()]
        );
    }
}
