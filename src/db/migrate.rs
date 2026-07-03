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
    (
        11,
        "default project lead",
        include_str!("../../migrations/011_default_project_lead.sql"),
    ),
    (
        12,
        "page comments",
        include_str!("../../migrations/012_page_comments.sql"),
    ),
    (
        13,
        "page labels",
        include_str!("../../migrations/013_page_labels.sql"),
    ),
    (
        14,
        "oauth user binding",
        include_str!("../../migrations/014_oauth_user_id.sql"),
    ),
    (
        15,
        "module icon",
        include_str!("../../migrations/015_module_icon.sql"),
    ),
    (
        16,
        "page status",
        include_str!("../../migrations/016_page_status.sql"),
    ),
    (
        17,
        "issue activity triggers",
        include_str!("../../migrations/017_issue_activity_triggers.sql"),
    ),
    (
        18,
        "audit log",
        include_str!("../../migrations/018_audit_log.sql"),
    ),
    (19, "plans", include_str!("../../migrations/019_plans.sql")),
    (
        20,
        "plans cascade",
        include_str!("../../migrations/020_plans_cascade.sql"),
    ),
    (
        21,
        "plans audit",
        include_str!("../../migrations/021_plans_audit.sql"),
    ),
    (
        22,
        "page pinned",
        include_str!("../../migrations/022_page_pinned.sql"),
    ),
    (
        23,
        "instance settings",
        include_str!("../../migrations/023_instance_settings.sql"),
    ),
    (
        24,
        "web auto-login",
        include_str!("../../migrations/024_web_auto_login.sql"),
    ),
    (
        25,
        "project sort order",
        include_str!("../../migrations/025_project_sort_order.sql"),
    ),
    (
        26,
        "project members",
        include_str!("../../migrations/026_project_members.sql"),
    ),
    (
        27,
        "authz enforced flag",
        include_str!("../../migrations/027_authz_enforced.sql"),
    ),
    (
        28,
        "project members audit",
        include_str!("../../migrations/028_project_members_audit.sql"),
    ),
    (
        29,
        "saved views",
        include_str!("../../migrations/029_saved_views.sql"),
    ),
    (
        30,
        "oauth device codes",
        include_str!("../../migrations/030_oauth_device_codes.sql"),
    ),
    (
        31,
        "attachments",
        include_str!("../../migrations/031_attachments.sql"),
    ),
    (
        32,
        "comment mentions",
        include_str!("../../migrations/032_comment_mentions.sql"),
    ),
    (
        33,
        "import source markers",
        include_str!("../../migrations/033_import_source.sql"),
    ),
    (
        34,
        "comment search",
        include_str!("../../migrations/034_comment_search.sql"),
    ),
];

/// Highest migration version this binary knows how to apply. Used by
/// `lific dump`/`restore` (LIF-266) to stamp and gate archives on schema
/// compatibility: an archive whose `schema_version` exceeds this was created
/// by a newer Lific and must not be restored onto an older binary.
pub fn latest_version() -> i64 {
    MIGRATIONS.iter().map(|(v, _, _)| *v).max().unwrap_or(0)
}

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
