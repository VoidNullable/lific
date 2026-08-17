use rusqlite::Connection;
use sha2::{Digest, Sha256};
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
    (
        35,
        "project groups",
        include_str!("../../migrations/035_project_groups.sql"),
    ),
    (
        36,
        "oauth client tool",
        include_str!("../../migrations/036_oauth_client_tool.sql"),
    ),
    (
        37,
        "users tool id",
        include_str!("../../migrations/037_users_tool_id.sql"),
    ),
    (
        38,
        "bot identity unique",
        include_str!("../../migrations/038_bot_identity_unique.sql"),
    ),
    (
        39,
        "project identifier nocase",
        include_str!("../../migrations/039_project_identifier_nocase.sql"),
    ),
    (
        40,
        "user active flag",
        include_str!("../../migrations/040_user_active.sql"),
    ),
    (
        41,
        "attachment metadata",
        include_str!("../../migrations/041_attachment_metadata.sql"),
    ),
];

/// Migrations that rebuild a table other tables reference by foreign key.
///
/// SQLite cannot change a column's collating sequence in place, so those
/// migrations drop and recreate the table. Two connection pragmas have to be
/// set around the rebuild, and neither can be set from inside the migration
/// file itself:
///
/// * `foreign_keys = OFF` — the pragma is a silent no-op inside a transaction
///   (and the migration runs in a savepoint). Without it, `DROP TABLE parent`
///   performs an implicit `DELETE FROM` that fires `ON DELETE CASCADE` and
///   takes every child row with it.
/// * `legacy_alter_table = ON` — otherwise `ALTER TABLE x RENAME TO parent`
///   reparses every trigger in the schema and fails with "error in trigger
///   audit_issues_insert: no such table: main.projects", since the old table
///   is already gone by then. Legacy mode also leaves child `REFERENCES`
///   clauses alone, which is what we want: ids are preserved verbatim.
///
/// This is the standard SQLite table-rebuild procedure. Both pragmas are
/// silent no-ops inside a transaction, and `run` serializes the whole batch
/// under one `BEGIN IMMEDIATE` (see below), so when a rebuild migration is
/// pending the pragmas are set on the connection *before* that transaction
/// and hold for the entire batch. Each migration still runs inside its own
/// savepoint, the rebuild verifies itself with `PRAGMA foreign_key_check`
/// before its savepoint releases, and `run_inner` repeats the check
/// batch-wide before commit to cover every other migration that ran while
/// enforcement was off.
const FK_REBUILD_MIGRATIONS: &[i64] = &[39];

/// Highest migration version this binary knows how to apply. Used by
/// `lific dump`/`restore` (LIF-266) to stamp and gate archives on schema
/// compatibility: an archive whose `schema_version` exceeds this was created
/// by a newer Lific and must not be restored onto an older binary.
pub fn latest_version() -> i64 {
    MIGRATIONS.iter().map(|(v, _, _)| *v).max().unwrap_or(0)
}

/// Integrity digest of a migration's SQL, as stored in `_migrations.checksum`.
///
/// SHA-256 over the migration text with `\r\n` folded to `\n`. The
/// normalization matters because `include_str!` embeds the file's bytes
/// verbatim: a checkout with CRLF line endings (Windows, or `core.autocrlf`)
/// would otherwise produce a different digest for byte-identical SQL and make
/// the same database fail verification depending on which machine built the
/// binary. This is an integrity check against accidentally edited history, not
/// a defense against an attacker who already controls the database file.
fn checksum(sql: &str) -> String {
    let normalized = sql.replace("\r\n", "\n");
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

/// Short form used in operator-facing messages; the full 64 hex characters
/// carry no extra meaning for a human comparing two values.
fn short(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

/// Ensure the migrations table exists and apply any pending migrations.
pub fn run(conn: &Connection) -> Result<(), crate::error::LificError> {
    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let needs_fk_rebuild = MIGRATIONS.iter().any(|(version, _, _)| {
        *version > current_version && FK_REBUILD_MIGRATIONS.contains(version)
    });

    // SQLite ignores PRAGMA foreign_keys changes inside a transaction, so
    // when the batch includes an FK-rebuild migration the connection has to
    // be configured before the serializing transaction below begins. See
    // `FK_REBUILD_MIGRATIONS` for why the rebuild needs these, and note the
    // batch-wide `foreign_key_check` in `run_inner` that compensates for the
    // widened enforcement-off window.
    if needs_fk_rebuild {
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             PRAGMA legacy_alter_table = ON;",
        )?;
    }

    // Serialize migration discovery and application across processes. Without
    // an IMMEDIATE transaction, two fresh connections can observe the same
    // version and both execute the next migration, racing on the migrations
    // table (notably on Windows where test/process startup is more concurrent).
    let transaction_result: Result<(), crate::error::LificError> =
        match conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => match run_inner(conn, needs_fk_rebuild) {
                Ok(()) => conn.execute_batch("COMMIT").map_err(Into::into),
                Err(error) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(error)
                }
            },
            Err(error) => Err(error.into()),
        };

    if needs_fk_rebuild {
        let restore_result = conn
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA legacy_alter_table = OFF;",
            )
            .map_err(Into::into);
        match transaction_result {
            Ok(()) => restore_result,
            Err(error) => {
                let _ = restore_result;
                Err(error)
            }
        }
    } else {
        transaction_result
    }
}

/// Returns true when `PRAGMA foreign_key_check` reports at least one
/// dangling reference. Works inside a transaction (unlike toggling
/// `PRAGMA foreign_keys` itself).
fn has_fk_violations(conn: &Connection) -> Result<bool, crate::error::LificError> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    Ok(rows.next()?.is_some())
}

fn run_inner(conn: &Connection, fk_off: bool) -> Result<(), crate::error::LificError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now')),
            checksum   TEXT
        );",
    )?;

    // `checksum` post-dates the table, so a database created by an older
    // Lific has the three-column shape and `CREATE TABLE IF NOT EXISTS` above
    // was a no-op for it. Add the column in place; rows keep NULL until the
    // backfill below stamps them.
    let has_checksum: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('_migrations') WHERE name = 'checksum'",
        [],
        |row| row.get(0),
    )?;
    if has_checksum == 0 {
        conn.execute_batch("ALTER TABLE _migrations ADD COLUMN checksum TEXT;")?;
    }

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // LIF-404: refuse to run against a schema this binary does not know.
    // Deploys swap the binary in place over a live database, so rolling back
    // to an older release points old code at a newer schema. Every migration
    // is already stamped, so the loop below is a no-op and the instance comes
    // up serving (and writing) a schema it was never compiled against.
    let latest = latest_version();
    if current_version > latest {
        return Err(crate::error::LificError::Internal(format!(
            "database schema version {current_version} is newer than this binary supports \
             (it knows migrations up to version {latest}). The database was written by a \
             newer Lific; running this binary against it would read and write an \
             unsupported schema. Use a Lific binary whose schema version is {current_version} \
             or higher, or restore a backup taken before the upgrade."
        )));
    }

    verify_checksums(conn)?;

    for &(version, name, sql) in MIGRATIONS {
        if version > current_version {
            info!(version, name, "applying migration");
            let sp = format!("migrate_v{version}");
            crate::db::queries::savepoint(conn, &sp, || {
                conn.execute_batch(sql)?;
                // A rebuild migration ran with enforcement off (see `run`);
                // check it here so an orphaning bug rolls back just this
                // savepoint with an error naming the migration.
                if FK_REBUILD_MIGRATIONS.contains(&version) && has_fk_violations(conn)? {
                    return Err(crate::error::LificError::Internal(format!(
                        "migration {version} ({name}) left dangling foreign key references"
                    )));
                }
                conn.execute(
                    "INSERT INTO _migrations (version, name, checksum) VALUES (?1, ?2, ?3)",
                    rusqlite::params![version, name, checksum(sql)],
                )?;
                Ok(())
            })?;
        }
    }

    // When a rebuild was pending, *every* migration in this batch ran with
    // foreign-key enforcement off, not just the rebuild (the pragma cannot
    // be toggled inside the serializing transaction). The rebuild checked
    // itself above; this sweep catches any other migration that would have
    // violated a constraint, before the batch commits.
    if fk_off && has_fk_violations(conn)? {
        return Err(crate::error::LificError::Internal(
            "migrations left dangling foreign key references".to_string(),
        ));
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

/// Check every already-applied migration against the SQL compiled into this
/// binary (LIF-415).
///
/// Migration files are history: once applied, their text describes what a
/// database actually went through. Editing one after the fact used to be
/// invisible, because the version is stamped, so the runner skips it and the
/// database quietly diverges from what the source tree claims it is. Storing a
/// digest turns that into a startup failure instead.
///
/// Rows whose checksum is NULL (every row in a database created before the
/// column existed) are backfilled with the current hash rather than treated as
/// a mismatch: there is nothing to compare against, and failing every existing
/// deployment on upgrade would be worse than starting the guarantee from here.
/// Rows for versions this binary doesn't carry are left alone; the
/// downgrade guard in `run` has already rejected the case that matters.
fn verify_checksums(conn: &Connection) -> Result<(), crate::error::LificError> {
    let applied: Vec<(i64, String, Option<String>)> = {
        let mut stmt = conn.prepare("SELECT version, name, checksum FROM _migrations")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut backfilled = 0usize;
    for (version, name, stored) in applied {
        let Some(&(_, _, sql)) = MIGRATIONS.iter().find(|(v, _, _)| *v == version) else {
            continue;
        };
        let expected = checksum(sql);
        match stored {
            None => {
                conn.execute(
                    "UPDATE _migrations SET checksum = ?1 WHERE version = ?2",
                    rusqlite::params![expected, version],
                )?;
                backfilled += 1;
            }
            Some(stored) if stored != expected => {
                return Err(crate::error::LificError::Internal(format!(
                    "migration {version} ({name}) has changed since it was applied to this \
                     database: recorded checksum {}, this binary's copy hashes to {}. Applied \
                     migrations are history and must not be edited. Add a new migration \
                     instead, or restore the original file.",
                    short(&stored),
                    short(&expected),
                )));
            }
            Some(_) => {}
        }
    }

    if backfilled > 0 {
        info!(
            backfilled,
            "recorded checksums for migrations applied before checksums existed"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database in the exact state a real instance is in just before
    /// migration `stop`: every earlier migration applied and stamped. The
    /// pool helpers (`db::open_memory`) always run the full set, so this is
    /// the only way to exercise an upgrade path's data handling.
    fn migrated_up_to(stop: i64) -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(
            "CREATE TABLE _migrations (
                version    INTEGER PRIMARY KEY,
                name       TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        for &(version, name, sql) in MIGRATIONS {
            if version >= stop {
                break;
            }
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("migration {version} ({name}): {e}"));
            conn.execute(
                "INSERT INTO _migrations (version, name) VALUES (?1, ?2)",
                rusqlite::params![version, name],
            )
            .unwrap();
        }
        conn
    }

    /// A fresh in-memory database with every migration applied by `run`
    /// itself, which is the state a real instance boots into.
    fn fully_migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run(&conn).expect("initial migration");
        conn
    }

    fn stored_checksum(conn: &Connection, version: i64) -> Option<String> {
        conn.query_row(
            "SELECT checksum FROM _migrations WHERE version = ?1",
            rusqlite::params![version],
            |row| row.get(0),
        )
        .unwrap()
    }

    // ── LIF-404: downgrade guard ──────────────────────────────────────────

    /// Deploys swap the binary in place, so rolling back to an older release
    /// aims old code at a newer schema. Every migration it knows is already
    /// stamped, so without this guard the runner does nothing and the
    /// instance serves and writes a schema it was never compiled against.
    #[test]
    fn run_refuses_a_database_newer_than_this_binary() {
        let conn = fully_migrated();
        let newer = latest_version() + 1;
        conn.execute(
            "INSERT INTO _migrations (version, name, checksum) VALUES (?1, 'from the future', 'x')",
            rusqlite::params![newer],
        )
        .unwrap();

        let err = run(&conn).expect_err("a newer schema must not be accepted");
        let msg = err.to_string();
        assert!(
            msg.contains(&newer.to_string()) && msg.contains(&latest_version().to_string()),
            "the error must name both versions: {msg}"
        );
        assert!(
            msg.contains("newer than this binary supports"),
            "the error must say what went wrong: {msg}"
        );
    }

    /// The guard only fires on a genuinely newer schema: re-running against a
    /// database this binary produced is the normal startup path.
    #[test]
    fn run_is_idempotent_on_a_database_at_this_binarys_version() {
        let conn = fully_migrated();
        run(&conn).expect("a second run must be a no-op");
        let max: i64 = conn
            .query_row("SELECT MAX(version) FROM _migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(max, latest_version());
    }

    // ── LIF-415: migration checksums ──────────────────────────────────────

    #[test]
    fn every_applied_migration_records_a_checksum() {
        let conn = fully_migrated();
        let missing: i64 = conn
            .query_row(
                "SELECT count(*) FROM _migrations WHERE checksum IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0, "every applied migration must be stamped");
        assert_eq!(
            stored_checksum(&conn, 1).as_deref(),
            Some(checksum(MIGRATIONS[0].2).as_str()),
            "the stamp must be the hash of the SQL that ran"
        );
    }

    /// Databases created before the column existed carry the three-column
    /// `_migrations` table and NULL checksums. Upgrading must add the column
    /// and backfill, not fail every existing deployment.
    #[test]
    fn legacy_databases_gain_the_column_and_get_backfilled() {
        let conn = fully_migrated();
        // Rebuild `_migrations` in its pre-checksum, three-column shape with
        // every migration stamped, which is what a live instance looks like
        // just before this upgrade.
        conn.execute_batch(
            "DROP TABLE _migrations;
             CREATE TABLE _migrations (
                 version    INTEGER PRIMARY KEY,
                 name       TEXT NOT NULL,
                 applied_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        for &(version, name, _) in MIGRATIONS {
            conn.execute(
                "INSERT INTO _migrations (version, name) VALUES (?1, ?2)",
                rusqlite::params![version, name],
            )
            .unwrap();
        }

        let has_column: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('_migrations') WHERE name = 'checksum'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_column, 0, "fixture must start on the pre-checksum shape");

        run(&conn).expect("a legacy database must upgrade cleanly");

        let missing: i64 = conn
            .query_row(
                "SELECT count(*) FROM _migrations WHERE checksum IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0, "NULL checksums must be backfilled");
        assert_eq!(
            stored_checksum(&conn, 1).as_deref(),
            Some(checksum(MIGRATIONS[0].2).as_str())
        );
        // And the backfilled stamps hold on the next boot.
        run(&conn).expect("backfilled checksums must verify");
    }

    /// Editing an already-applied migration file is invisible without this:
    /// the version is stamped, so the runner skips it and the database
    /// silently disagrees with the source tree.
    #[test]
    fn run_fails_when_an_applied_migrations_sql_has_changed() {
        let conn = fully_migrated();
        // Stand in for an edited file by recording a different hash than the
        // compiled-in SQL produces.
        conn.execute(
            "UPDATE _migrations SET checksum = ?1 WHERE version = 17",
            rusqlite::params![checksum("-- someone edited this later")],
        )
        .unwrap();

        let err = run(&conn).expect_err("an edited migration must stop startup");
        let msg = err.to_string();
        assert!(
            msg.contains("migration 17 (issue activity triggers)"),
            "the error must name the migration: {msg}"
        );
        assert!(
            msg.contains("has changed since it was applied"),
            "the error must say what went wrong: {msg}"
        );
    }

    /// Line endings are a checkout detail, not a change to the SQL: a CRLF
    /// working copy must not make a database built on LF fail verification.
    #[test]
    fn checksum_ignores_line_ending_style() {
        assert_eq!(
            checksum("CREATE TABLE t (a);\nCREATE INDEX i ON t (a);\n"),
            checksum("CREATE TABLE t (a);\r\nCREATE INDEX i ON t (a);\r\n")
        );
        assert_ne!(checksum("SELECT 1;"), checksum("SELECT 2;"));
    }

    fn identifiers(conn: &Connection) -> Vec<(i64, String)> {
        let mut stmt = conn
            .prepare("SELECT id, identifier FROM projects ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        rows.collect::<Result<Vec<_>, _>>().unwrap()
    }

    /// LIF-348: migration 039 puts a NOCASE unique index on
    /// `projects.identifier`, so a legacy database holding both `ABCDE` and
    /// `abcde` has to be deduplicated on the way through — and the migration
    /// runs at startup, so a UNIQUE violation here would be an instance that
    /// refuses to boot.
    ///
    /// The two collision groups are the case an earlier `base || rn` scheme
    /// got wrong: `ABCDE`/`abcde` and `ABCDF`/`abcdf` both truncate to
    /// `ABCD2` at the 5-character limit, so the two *generated* names
    /// collided with each other even though neither collided with anything
    /// that already existed. `P1` and `P2` are seeded to push the synthetic
    /// names off their first choice as well.
    #[test]
    fn project_identifier_rebuild_resolves_case_collisions_without_losing_rows() {
        let conn = migrated_up_to(39);
        conn.execute_batch(
            "INSERT INTO projects (id, name, identifier) VALUES
                 (1, 'Upper E',  'ABCDE'),
                 (2, 'Lower E',  'abcde'),
                 (3, 'Upper F',  'ABCDF'),
                 (4, 'Lower F',  'abcdf'),
                 (5, 'Taken P1', 'P1'),
                 (6, 'Taken P2', 'P2'),
                 (7, 'Lific',    'LIF');
             INSERT INTO issues (project_id, sequence, title) VALUES
                 (1, 1, 'keeps its project'),
                 (2, 1, 'renamed project'),
                 (4, 1, 'renamed project'),
                 (7, 1, 'untouched project');
             INSERT INTO modules (project_id, name) VALUES (2, 'a module');",
        )
        .unwrap();

        run(&conn).expect("migration 039 must not abort on case collisions");

        // Oldest row of each group keeps its identifier; the later colliders
        // (ids 2 and 4, ranked k=1 and k=2) land on Q1/Q2 because P1/P2 are
        // already taken.
        assert_eq!(
            identifiers(&conn),
            vec![
                (1, "ABCDE".to_string()),
                (2, "Q1".to_string()),
                (3, "ABCDF".to_string()),
                (4, "Q2".to_string()),
                (5, "P1".to_string()),
                (6, "P2".to_string()),
                (7, "LIF".to_string()),
            ]
        );

        // Nothing was cascaded away by the rebuild's DROP TABLE.
        let issues: i64 = conn
            .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
            .unwrap();
        let modules: i64 = conn
            .query_row("SELECT count(*) FROM modules", [], |row| row.get(0))
            .unwrap();
        assert_eq!((issues, modules), (4, 1));

        // Identifiers are unique case-insensitively, and lookups now are too.
        let distinct: i64 = conn
            .query_row(
                "SELECT count(DISTINCT lower(identifier)) FROM projects",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(distinct, 7);
        for spelling in ["LIF", "lif", "Lif"] {
            let id: i64 = conn
                .query_row(
                    "SELECT id FROM projects WHERE identifier = ?1",
                    rusqlite::params![spelling],
                    |row| row.get(0),
                )
                .unwrap_or_else(|e| panic!("{spelling} should resolve: {e}"));
            assert_eq!(id, 7);
        }

        // The rebuild's pragmas were restored: enforcement is back on, so a
        // dangling reference is rejected and a delete still cascades.
        let fk_on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fk_on, 1);
        assert!(
            conn.execute(
                "INSERT INTO issues (project_id, sequence, title) VALUES (999, 1, 'orphan')",
                [],
            )
            .is_err(),
            "foreign key enforcement must be restored after the rebuild"
        );
        conn.execute("DELETE FROM projects WHERE id = 2", [])
            .unwrap();
        let issues: i64 = conn
            .query_row("SELECT count(*) FROM issues", [], |row| row.get(0))
            .unwrap();
        let modules: i64 = conn
            .query_row("SELECT count(*) FROM modules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            (issues, modules),
            (3, 0),
            "ON DELETE CASCADE must still fire"
        );
    }

    /// The happy path: a database with no case collisions passes through the
    /// rebuild with every identifier byte-identical, and the new column
    /// collation makes an existing identifier unrepeatable in any casing.
    #[test]
    fn project_identifier_rebuild_is_a_no_op_without_collisions() {
        let conn = migrated_up_to(39);
        conn.execute_batch(
            "INSERT INTO projects (id, name, identifier, sort_order) VALUES
                 (1, 'Lific', 'LIF', 3),
                 (2, 'Other', 'OTH', 1);",
        )
        .unwrap();

        run(&conn).expect("migration 039");

        assert_eq!(
            identifiers(&conn),
            vec![(1, "LIF".to_string()), (2, "OTH".to_string())]
        );
        let sort_order: i64 = conn
            .query_row("SELECT sort_order FROM projects WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(sort_order, 3, "columns added by later migrations survive");
        assert!(
            conn.execute(
                "INSERT INTO projects (name, identifier) VALUES ('Dup', 'lif')",
                [],
            )
            .is_err(),
            "the unique index must now be case-insensitive"
        );
    }
}
