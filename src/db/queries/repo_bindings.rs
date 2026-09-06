//! Repo → project bindings (LIF-448, migration 049).
//!
//! A *binding* points one repository at one project. What identifies that
//! repository is not a single string but a set of *aliases*: the normalized
//! remote URL, the absolute worktree root, and whatever else a checkout can
//! present. Each alias is a `(kind, value)` row in `repo_identities` that
//! resolves to the binding.
//!
//! `UNIQUE(kind, value)` is enforced across the whole instance, so an alias
//! belongs to exactly one binding. Two consequences the callers rely on:
//!
//! * Claiming an alias another binding already owns fails loudly, as
//!   [`LificError::Conflict`], instead of silently repointing a checkout.
//! * [`merge`] can move every identity from one binding to another without
//!   ever colliding, because a collision would mean the alias was already
//!   claimed twice.
//!
//! [`resolve`] is the read the whole feature turns on: given the aliases a
//! checkout presents, it answers with [`Resolution`] — nothing, exactly one
//! binding, or several. The last case is a real state, not an error: two
//! bindings created independently (one from a remote, one from a path) can
//! both be presented by the same checkout later, and only a human can say
//! which is right. See [`merge`] for the resolution.
//!
//! Every mutation here is audited by the triggers in migration 049; nothing in
//! this module writes `audit_log` by hand.

// Storage layer for a feature whose API, CLI and MCP surfaces land in
// follow-up tasks — the whole module is currently exercised only by its own
// tests.
#![allow(dead_code)]

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::db::models::{RepoBinding, RepoIdentity};
use crate::error::LificError;

/// What a set of presented aliases resolved to.
#[derive(Debug)]
pub enum Resolution {
    /// No alias is known. The checkout has never been bound.
    None,
    /// Every alias that matched points at the same binding.
    One(RepoBinding),
    /// The aliases span more than one binding, ordered by id. Ambiguous by
    /// construction: the caller must pick, or [`merge`] them.
    Conflict(Vec<RepoBinding>),
}

fn row_to_binding(row: &rusqlite::Row) -> rusqlite::Result<RepoBinding> {
    Ok(RepoBinding {
        id: row.get(0)?,
        project_id: row.get(1)?,
        created_at: row.get(2)?,
        created_by: row.get(3)?,
    })
}

fn row_to_identity(row: &rusqlite::Row) -> rusqlite::Result<RepoIdentity> {
    Ok(RepoIdentity {
        id: row.get(0)?,
        binding_id: row.get(1)?,
        kind: row.get(2)?,
        value: row.get(3)?,
        first_seen_at: row.get(4)?,
    })
}

/// Map a constraint violation from an identity write onto an error the caller
/// can act on. The three the schema can produce are genuinely different
/// answers, so they are read off the *extended* result code rather than the
/// coarse `ConstraintViolation`:
///
/// * `UNIQUE` — another binding already owns this alias (409).
/// * `CHECK`  — `kind` is not `remote` or `root` (400).
/// * `FOREIGN KEY` — no such binding (404).
fn identity_constraint<'a>(
    binding_id: i64,
    kind: &'a str,
    value: &'a str,
) -> impl Fn(rusqlite::Error) -> LificError + 'a {
    move |e| match &e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            match err.extended_code {
                rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY => LificError::Conflict(format!(
                    "repo identity {kind} '{value}' is already claimed by another binding"
                )),
                rusqlite::ffi::SQLITE_CONSTRAINT_CHECK => LificError::BadRequest(format!(
                    "'{kind}' is not a valid repo identity kind (expected 'remote' or 'root')"
                )),
                rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => {
                    LificError::NotFound(format!("repo binding {binding_id} not found"))
                }
                _ => e.into(),
            }
        }
        _ => e.into(),
    }
}

/// Map a constraint violation from a binding write. Only the project FK can
/// fire here, and it means the target project does not exist.
fn binding_constraint(project_id: i64) -> impl Fn(rusqlite::Error) -> LificError {
    move |e| match &e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation
                && err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
        {
            LificError::NotFound(format!("project {project_id} not found"))
        }
        _ => e.into(),
    }
}

/// Insert a binding for `project_id` plus the aliases that resolve to it.
///
/// Runs in a savepoint: a rejected alias (one already claimed, or a bad
/// `kind`) must not leave a bindingless-alias row or an aliasless binding
/// behind. Concurrency is not a concern on top of that — the caller holds the
/// single write connection for the whole call.
///
/// An empty `aliases` list is accepted and creates a binding nothing resolves
/// to, reachable only by id.
pub fn create_binding(
    conn: &Connection,
    project_id: i64,
    created_by: Option<i64>,
    aliases: &[(&str, &str)],
) -> Result<RepoBinding, LificError> {
    super::savepoint(conn, "repo_binding_create", || {
        conn.execute(
            "INSERT INTO repo_bindings (project_id, created_by) VALUES (?1, ?2)",
            params![project_id, created_by],
        )
        .map_err(binding_constraint(project_id))?;
        let id = conn.last_insert_rowid();

        for (kind, value) in aliases {
            insert_identity(conn, id, kind, value)?;
        }

        get(conn, id)?
            .ok_or_else(|| LificError::Internal(format!("repo binding {id} vanished after insert")))
    })
}

/// The bindings any of `aliases` resolves to.
///
/// Matching is on the whole `(kind, value)` pair, so a path that happens to
/// equal a remote URL cannot cross-match. Distinct bindings only: a checkout
/// presenting both its remote and its root, both already on one binding, is
/// [`Resolution::One`], not a conflict.
pub fn resolve(conn: &Connection, aliases: &[(&str, &str)]) -> Result<Resolution, LificError> {
    if aliases.is_empty() {
        return Ok(Resolution::None);
    }

    let predicate = vec!["(i.kind = ? AND i.value = ?)"; aliases.len()].join(" OR ");
    let sql = format!(
        "SELECT DISTINCT b.id, b.project_id, b.created_at, b.created_by
         FROM repo_bindings b
         JOIN repo_identities i ON i.binding_id = b.id
         WHERE {predicate}
         ORDER BY b.id"
    );

    let mut binds: Vec<&str> = Vec::with_capacity(aliases.len() * 2);
    for (kind, value) in aliases {
        binds.push(kind);
        binds.push(value);
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds), row_to_binding)?;
    let mut found: Vec<RepoBinding> = Vec::new();
    for row in rows {
        found.push(row?);
    }

    Ok(match found.len() {
        0 => Resolution::None,
        1 => Resolution::One(found.remove(0)),
        _ => Resolution::Conflict(found),
    })
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<RepoBinding>, LificError> {
    Ok(conn
        .query_row(
            "SELECT id, project_id, created_at, created_by FROM repo_bindings WHERE id = ?1",
            params![id],
            row_to_binding,
        )
        .optional()?)
}

/// The aliases on a binding, oldest first.
pub fn list_identities(
    conn: &Connection,
    binding_id: i64,
) -> Result<Vec<RepoIdentity>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, binding_id, kind, value, first_seen_at
         FROM repo_identities WHERE binding_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![binding_id], row_to_identity)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Every binding pointing at `project_id`, oldest first. More than one is
/// normal: several unrelated checkouts can file against the same project.
pub fn list_for_project(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<RepoBinding>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, created_at, created_by
         FROM repo_bindings WHERE project_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![project_id], row_to_binding)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Insert one alias row. Shared by [`create_binding`] and [`add_identity`];
/// neither reads it back, so the row-fetch stays with the public function.
fn insert_identity(
    conn: &Connection,
    binding_id: i64,
    kind: &str,
    value: &str,
) -> Result<i64, LificError> {
    conn.execute(
        "INSERT INTO repo_identities (binding_id, kind, value) VALUES (?1, ?2, ?3)",
        params![binding_id, kind, value],
    )
    .map_err(identity_constraint(binding_id, kind, value))?;
    Ok(conn.last_insert_rowid())
}

/// Teach an existing binding one more alias — the second clone of a repo
/// already bound through its remote, say.
pub fn add_identity(
    conn: &Connection,
    binding_id: i64,
    kind: &str,
    value: &str,
) -> Result<RepoIdentity, LificError> {
    let id = insert_identity(conn, binding_id, kind, value)?;
    conn.query_row(
        "SELECT id, binding_id, kind, value, first_seen_at FROM repo_identities WHERE id = ?1",
        params![id],
        row_to_identity,
    )
    .map_err(Into::into)
}

/// Drop one alias. `false` means there was no such identity.
pub fn remove_identity(conn: &Connection, identity_id: i64) -> Result<bool, LificError> {
    Ok(conn.execute(
        "DELETE FROM repo_identities WHERE id = ?1",
        params![identity_id],
    )? > 0)
}

/// Point a binding at a different project, keeping its aliases. `false` means
/// there was no such binding.
pub fn repoint(
    conn: &Connection,
    binding_id: i64,
    new_project_id: i64,
) -> Result<bool, LificError> {
    Ok(conn
        .execute(
            "UPDATE repo_bindings SET project_id = ?2 WHERE id = ?1",
            params![binding_id, new_project_id],
        )
        .map_err(binding_constraint(new_project_id))?
        > 0)
}

/// Delete a binding and, by cascade, its aliases. `false` means there was no
/// such binding.
pub fn delete_binding(conn: &Connection, binding_id: i64) -> Result<bool, LificError> {
    Ok(conn.execute(
        "DELETE FROM repo_bindings WHERE id = ?1",
        params![binding_id],
    )? > 0)
}

/// Fold `from_binding_id` into `into_binding_id`: every alias moves across,
/// then the emptied binding is deleted. The cure for
/// [`Resolution::Conflict`].
///
/// Both ids must exist and must differ. The alias move cannot collide, since
/// `UNIQUE(kind, value)` already guarantees no alias sits on both bindings.
pub fn merge(
    conn: &Connection,
    from_binding_id: i64,
    into_binding_id: i64,
) -> Result<(), LificError> {
    if from_binding_id == into_binding_id {
        return Err(LificError::BadRequest(
            "cannot merge a repo binding into itself".into(),
        ));
    }
    for id in [from_binding_id, into_binding_id] {
        if get(conn, id)?.is_none() {
            return Err(LificError::NotFound(format!("repo binding {id} not found")));
        }
    }

    super::savepoint(conn, "repo_binding_merge", || {
        conn.execute(
            "UPDATE repo_identities SET binding_id = ?2 WHERE binding_id = ?1",
            params![from_binding_id, into_binding_id],
        )?;
        conn.execute(
            "DELETE FROM repo_bindings WHERE id = ?1",
            params![from_binding_id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn pool() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn project(conn: &Connection, identifier: &str) -> i64 {
        conn.execute(
            "INSERT INTO projects (name, identifier, description) VALUES (?1, ?1, '')",
            params![identifier],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn audit_rows(conn: &Connection, entity_type: &str, action: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM audit_log WHERE entity_type = ?1 AND action = ?2",
            params![entity_type, action],
            |row| row.get(0),
        )
        .unwrap()
    }

    // ── creating ──────────────────────────────────────────────

    #[test]
    fn a_binding_can_carry_a_single_alias() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "ONE");

        let binding = create_binding(&c, proj, None, &[("remote", "github.com/void/one")]).unwrap();

        assert_eq!(binding.project_id, proj);
        assert!(binding.created_by.is_none());
        let aliases = list_identities(&c, binding.id).unwrap();
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases[0].kind, "remote");
        assert_eq!(aliases[0].value, "github.com/void/one");
        assert_eq!(aliases[0].binding_id, binding.id);
    }

    #[test]
    fn a_binding_can_carry_a_remote_and_a_root() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "TWO");

        let binding = create_binding(
            &c,
            proj,
            Some(7),
            &[("remote", "github.com/void/two"), ("root", "/mnt/dev/two")],
        )
        .unwrap();

        assert_eq!(binding.created_by, Some(7));
        let kinds: Vec<String> = list_identities(&c, binding.id)
            .unwrap()
            .into_iter()
            .map(|i| i.kind)
            .collect();
        assert_eq!(kinds, vec!["remote", "root"]);
    }

    #[test]
    fn creating_a_binding_for_an_unknown_project_is_not_found() {
        let pool = pool();
        let c = pool.write().unwrap();

        let err = create_binding(&c, 9999, None, &[("root", "/tmp/nope")]).unwrap_err();

        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── constraints ───────────────────────────────────────────

    #[test]
    fn a_second_binding_cannot_claim_an_alias_that_is_already_owned() {
        let pool = pool();
        let c = pool.write().unwrap();
        let first = project(&c, "FIRST");
        let second = project(&c, "SECOND");
        create_binding(&c, first, None, &[("remote", "github.com/void/shared")]).unwrap();

        let err =
            create_binding(&c, second, None, &[("remote", "github.com/void/shared")]).unwrap_err();

        assert!(
            matches!(err, LificError::Conflict(_)),
            "a claimed alias must be distinguishable as a conflict, got {err:?}"
        );
        // …and the rejected create left nothing behind.
        assert!(list_for_project(&c, second).unwrap().is_empty());
    }

    #[test]
    fn add_identity_rejects_an_alias_another_binding_owns() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "DUP");
        let a = create_binding(&c, proj, None, &[("root", "/mnt/dev/a")]).unwrap();
        let b = create_binding(&c, proj, None, &[("root", "/mnt/dev/b")]).unwrap();

        let err = add_identity(&c, b.id, "root", "/mnt/dev/a").unwrap_err();

        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        assert_eq!(list_identities(&c, a.id).unwrap().len(), 1);
    }

    #[test]
    fn kind_must_be_remote_or_root() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "KIND");
        let binding = create_binding(&c, proj, None, &[("root", "/mnt/dev/kind")]).unwrap();

        // Through the query layer…
        let err = add_identity(&c, binding.id, "banana", "/mnt/dev/kind2").unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");

        // …and at the schema level, so a raw writer cannot sneak one in.
        assert!(
            c.execute(
                "INSERT INTO repo_identities (binding_id, kind, value) VALUES (?1, 'banana', 'x')",
                params![binding.id],
            )
            .is_err(),
            "the CHECK constraint must reject an unknown kind"
        );
    }

    #[test]
    fn add_identity_on_an_unknown_binding_is_not_found() {
        let pool = pool();
        let c = pool.write().unwrap();

        let err = add_identity(&c, 4242, "root", "/mnt/dev/ghost").unwrap_err();

        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── cascade ───────────────────────────────────────────────

    #[test]
    fn deleting_the_project_removes_its_bindings_and_their_identities() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "GONE");
        let binding = create_binding(
            &c,
            proj,
            None,
            &[
                ("remote", "github.com/void/gone"),
                ("root", "/mnt/dev/gone"),
            ],
        )
        .unwrap();

        c.execute("DELETE FROM projects WHERE id = ?1", params![proj])
            .unwrap();

        assert!(get(&c, binding.id).unwrap().is_none());
        assert!(list_identities(&c, binding.id).unwrap().is_empty());
        let identities: i64 = c
            .query_row("SELECT COUNT(*) FROM repo_identities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(identities, 0, "identities must not outlive their binding");
    }

    #[test]
    fn deleting_a_binding_removes_its_identities() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "DROP");
        let binding = create_binding(&c, proj, None, &[("root", "/mnt/dev/drop")]).unwrap();

        assert!(delete_binding(&c, binding.id).unwrap());
        assert!(!delete_binding(&c, binding.id).unwrap(), "already gone");
        assert!(list_identities(&c, binding.id).unwrap().is_empty());
    }

    // ── resolve ───────────────────────────────────────────────

    #[test]
    fn resolve_finds_nothing_for_an_unknown_checkout() {
        let pool = pool();
        let c = pool.write().unwrap();

        assert!(matches!(
            resolve(&c, &[("root", "/mnt/dev/unbound")]).unwrap(),
            Resolution::None
        ));
        assert!(matches!(resolve(&c, &[]).unwrap(), Resolution::None));
    }

    #[test]
    fn resolve_matches_through_either_alias_and_stays_one_binding() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "RES");
        let binding = create_binding(
            &c,
            proj,
            None,
            &[("remote", "github.com/void/res"), ("root", "/mnt/dev/res")],
        )
        .unwrap();

        for aliases in [
            vec![("remote", "github.com/void/res")],
            vec![("root", "/mnt/dev/res")],
            // Both at once is still ONE binding, not a conflict.
            vec![("remote", "github.com/void/res"), ("root", "/mnt/dev/res")],
        ] {
            match resolve(&c, &aliases).unwrap() {
                Resolution::One(found) => assert_eq!(found.id, binding.id),
                other => panic!("expected one binding for {aliases:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn resolve_does_not_cross_match_a_root_against_a_remote() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "XKIND");
        create_binding(&c, proj, None, &[("remote", "/mnt/dev/xkind")]).unwrap();

        // Same value, different kind: no match.
        assert!(matches!(
            resolve(&c, &[("root", "/mnt/dev/xkind")]).unwrap(),
            Resolution::None
        ));
    }

    #[test]
    fn resolve_reports_a_conflict_when_the_aliases_span_two_bindings() {
        let pool = pool();
        let c = pool.write().unwrap();
        let one = project(&c, "CONE");
        let two = project(&c, "CTWO");
        let a = create_binding(&c, one, None, &[("remote", "github.com/void/c")]).unwrap();
        let b = create_binding(&c, two, None, &[("root", "/mnt/dev/c")]).unwrap();

        match resolve(
            &c,
            &[("remote", "github.com/void/c"), ("root", "/mnt/dev/c")],
        )
        .unwrap()
        {
            Resolution::Conflict(found) => {
                let ids: Vec<i64> = found.iter().map(|b| b.id).collect();
                assert_eq!(ids, vec![a.id, b.id], "ordered by binding id");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    // ── identities ────────────────────────────────────────────

    #[test]
    fn an_alias_can_be_added_and_removed() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "ALIAS");
        let binding = create_binding(&c, proj, None, &[("remote", "github.com/void/al")]).unwrap();

        let added = add_identity(&c, binding.id, "root", "/mnt/dev/al").unwrap();
        assert_eq!(added.binding_id, binding.id);
        assert_eq!(list_identities(&c, binding.id).unwrap().len(), 2);
        match resolve(&c, &[("root", "/mnt/dev/al")]).unwrap() {
            Resolution::One(found) => assert_eq!(found.id, binding.id),
            other => panic!("expected the binding, got {other:?}"),
        }

        assert!(remove_identity(&c, added.id).unwrap());
        assert!(!remove_identity(&c, added.id).unwrap(), "already gone");
        assert!(matches!(
            resolve(&c, &[("root", "/mnt/dev/al")]).unwrap(),
            Resolution::None
        ));
    }

    #[test]
    fn list_for_project_returns_every_binding_that_points_at_it() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "MANY");
        let other = project(&c, "OTHER");
        let a = create_binding(&c, proj, None, &[("root", "/mnt/dev/many-a")]).unwrap();
        let b = create_binding(&c, proj, None, &[("root", "/mnt/dev/many-b")]).unwrap();
        create_binding(&c, other, None, &[("root", "/mnt/dev/other")]).unwrap();

        let ids: Vec<i64> = list_for_project(&c, proj)
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect();

        assert_eq!(ids, vec![a.id, b.id]);
    }

    // ── merge / repoint ───────────────────────────────────────

    #[test]
    fn merging_moves_the_identities_and_retires_the_source() {
        let pool = pool();
        let c = pool.write().unwrap();
        let one = project(&c, "MONE");
        let two = project(&c, "MTWO");
        let from = create_binding(&c, one, None, &[("remote", "github.com/void/m")]).unwrap();
        let into = create_binding(&c, two, None, &[("root", "/mnt/dev/m")]).unwrap();

        merge(&c, from.id, into.id).unwrap();

        assert!(
            get(&c, from.id).unwrap().is_none(),
            "source binding is gone"
        );
        let values: Vec<String> = list_identities(&c, into.id)
            .unwrap()
            .into_iter()
            .map(|i| i.value)
            .collect();
        assert_eq!(values, vec!["github.com/void/m", "/mnt/dev/m"]);

        // The conflict that motivated the merge now resolves cleanly.
        match resolve(
            &c,
            &[("remote", "github.com/void/m"), ("root", "/mnt/dev/m")],
        )
        .unwrap()
        {
            Resolution::One(found) => assert_eq!(found.id, into.id),
            other => panic!("expected one binding after the merge, got {other:?}"),
        }
    }

    #[test]
    fn merging_rejects_an_unknown_or_self_target() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "MERR");
        let binding = create_binding(&c, proj, None, &[("root", "/mnt/dev/merr")]).unwrap();

        assert!(matches!(
            merge(&c, binding.id, binding.id).unwrap_err(),
            LificError::BadRequest(_)
        ));
        assert!(matches!(
            merge(&c, binding.id, 9999).unwrap_err(),
            LificError::NotFound(_)
        ));
        assert!(matches!(
            merge(&c, 9999, binding.id).unwrap_err(),
            LificError::NotFound(_)
        ));
        // Nothing was disturbed by the refusals.
        assert_eq!(list_identities(&c, binding.id).unwrap().len(), 1);
    }

    #[test]
    fn repointing_moves_a_binding_to_another_project() {
        let pool = pool();
        let c = pool.write().unwrap();
        let one = project(&c, "RONE");
        let two = project(&c, "RTWO");
        let binding = create_binding(&c, one, None, &[("root", "/mnt/dev/r")]).unwrap();

        assert!(repoint(&c, binding.id, two).unwrap());

        assert_eq!(get(&c, binding.id).unwrap().unwrap().project_id, two);
        assert!(
            !repoint(&c, 9999, two).unwrap(),
            "no such binding reports false, not an error"
        );
        assert!(matches!(
            repoint(&c, binding.id, 9999).unwrap_err(),
            LificError::NotFound(_)
        ));
        // The aliases came along.
        assert_eq!(list_identities(&c, binding.id).unwrap().len(), 1);
    }

    // ── audit ─────────────────────────────────────────────────

    #[test]
    fn creating_and_deleting_a_binding_is_audited() {
        let pool = pool();
        let c = pool.write().unwrap();
        let proj = project(&c, "AUD");

        let binding = create_binding(
            &c,
            proj,
            None,
            &[("remote", "github.com/void/aud"), ("root", "/mnt/dev/aud")],
        )
        .unwrap();

        assert_eq!(audit_rows(&c, "repo_binding", "create"), 1);
        assert_eq!(audit_rows(&c, "repo_identity", "create"), 2);

        // The create row carries the project scope and label the feed needs.
        let (label, scope): (Option<String>, Option<i64>) = c
            .query_row(
                "SELECT entity_label, project_id FROM audit_log
                 WHERE entity_type = 'repo_binding' AND entity_id = ?1 AND action = 'create'",
                params![binding.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(label.as_deref(), Some("AUD"));
        assert_eq!(scope, Some(proj));

        delete_binding(&c, binding.id).unwrap();

        assert_eq!(audit_rows(&c, "repo_binding", "delete"), 1);
        assert_eq!(
            audit_rows(&c, "repo_identity", "delete"),
            2,
            "cascaded identity deletes are audited too"
        );
    }

    #[test]
    fn repointing_is_audited_but_a_no_op_update_is_not() {
        let pool = pool();
        let c = pool.write().unwrap();
        let one = project(&c, "AONE");
        let two = project(&c, "ATWO");
        let binding = create_binding(&c, one, None, &[("root", "/mnt/dev/aud2")]).unwrap();

        repoint(&c, binding.id, two).unwrap();
        assert_eq!(audit_rows(&c, "repo_binding", "update"), 1);

        let (field, old, new): (Option<String>, Option<String>, Option<String>) = c
            .query_row(
                "SELECT field, old_value, new_value FROM audit_log
                 WHERE entity_type = 'repo_binding' AND action = 'update'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(field.as_deref(), Some("project"));
        assert_eq!(old.as_deref(), Some("AONE"), "resolved names, not ids");
        assert_eq!(new.as_deref(), Some("ATWO"));

        // Repointing at the project it already targets changes nothing, so it
        // must not manufacture a row.
        repoint(&c, binding.id, two).unwrap();
        assert_eq!(audit_rows(&c, "repo_binding", "update"), 1);
    }
}
