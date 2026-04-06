use rusqlite::Connection;
use tracing::info;

/// Migrations are applied in order and tracked in a `_migrations` table.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial schema",
        include_str!("../../migrations/001_initial.sql"),
    ),
    (
        2,
        "page identifiers",
        include_str!("../../migrations/002_page_identifiers.sql"),
    ),
    (
        3,
        "api keys",
        include_str!("../../migrations/003_api_keys.sql"),
    ),
    (4, "oauth", include_str!("../../migrations/004_oauth.sql")),
    (
        5,
        "users and sessions",
        include_str!("../../migrations/005_users.sql"),
    ),
    (
        6,
        "comments",
        include_str!("../../migrations/006_comments.sql"),
    ),
    (
        7,
        "bot owners",
        include_str!("../../migrations/007_bot_owners.sql"),
    ),
    (
        8,
        "project lead",
        include_str!("../../migrations/008_project_lead.sql"),
    ),
    (
        9,
        "oauth scope",
        include_str!("../../migrations/009_oauth_scope.sql"),
    ),
    (
        10,
        "api key id",
        include_str!("../../migrations/010_api_key_id.sql"),
    ),
];

/// Ensure the migrations table exists and apply any pending migrations.
pub fn run(conn: &Connection) -> Result<(), crate::error::LificError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    for &(version, name, sql) in MIGRATIONS {
        if version > current_version {
            info!(version, name, "applying migration");
            let sp = format!("migrate_v{version}");
            crate::db::queries::savepoint(conn, &sp, || {
                conn.execute_batch(sql)?;
                conn.execute(
                    "INSERT INTO _migrations (version, name) VALUES (?1, ?2)",
                    rusqlite::params![version, name],
                )?;
                Ok(())
            })?;
        }
    }

    if current_version == 0 {
        info!(
            total = MIGRATIONS.len(),
            "database initialized with all migrations"
        );
    } else {
        let applied = MIGRATIONS
            .iter()
            .filter(|(v, _, _)| *v > current_version)
            .count();
        if applied > 0 {
            info!(applied, "new migrations applied");
        }
    }

    Ok(())
}
