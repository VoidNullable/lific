//! `lific bind` — point the repository containing the current directory at a
//! project, or report what it already resolves to (LIF-450).
//!
//! The identity of a checkout is computed by [`crate::repo_identity`]: the
//! normalized `origin` remote and the root commit of HEAD's first-parent
//! chain. Neither depends on where the checkout happens to live, so a second
//! clone of the same repository resolves to the same project without being
//! bound again.
//!
//! Both backends run the same three-step shape: compute the aliases, decide
//! what they mean, print one answer. Only the middle step differs — the SQL
//! path talks to [`crate::db::queries::repo_bindings`] directly, the HTTP
//! path to `/api/repos/{resolve,bind}` — and both produce the identical JSON
//! document ([`bound_json`] and friends), so `--json` output does not depend
//! on the transport. The human rendering is [`human`], which reads that
//! document and nothing else.
//!
//! The HTTP path is the one that matters most: it is the only way an operator
//! of a remote instance can bind at all, because `lific connect` needs a local
//! database and a remote deployment has none.
//!
//! # JSON shape
//!
//! Every path emits exactly these keys:
//!
//! ```json
//! {
//!   "resolution": "none" | "one" | "conflict",
//!   "project": null | { "id": 1, "identifier": "LIF", "name": "Lific" },
//!   "projects": [ { "id": 2, "identifier": "OTH", "name": "Other" } ],
//!   "aliases": [ { "kind": "remote", "value": "v1:…", "matched": true } ],
//!   "created": false
//! }
//! ```
//!
//! * `resolution` — what the presented aliases point at. A successful bind is
//!   always `"one"`, since the repository resolves to exactly one project the
//!   moment the command returns.
//! * `project` — the resolved project, `null` unless `resolution` is `"one"`.
//! * `projects` — populated only for `"conflict"`, empty otherwise. It is a
//!   separate key rather than a polymorphic `project` so the type of each
//!   field is stable across every outcome.
//! * `aliases` — every alias this checkout presented, in identity order, each
//!   flagged with whether it resolves to the reported binding. An unmatched
//!   alias on a bound repository is normal (a repo bound through its remote
//!   before its root alias existed, say).
//! * `created` — whether this invocation created the project (`--create`).

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::db::DbPool;
use crate::db::models::{CreateProject, Project, RepoBinding};
use crate::db::queries::{
    self,
    repo_bindings::{self, Resolution},
};
use crate::error::LificError;
use crate::repo_identity::{self, AliasKind, RepoIdentityError};

/// A `(kind, value)` alias pair as the query layer and the API both spell it.
type Alias = (String, String);

const NOT_A_REPOSITORY: &str = "not inside a git repository: `lific bind` identifies the \
     repository that contains the current directory, so run it from inside a checkout";

const GIT_REQUIRED: &str = "`lific bind` identifies a repository by asking git about it, and the \
     `git` binary was not found on PATH; install git or run this from a machine that has it";

/// Zero aliases means both of the two possible identities are missing, so the
/// refusal names both and why each one is absent. There is nothing to bind
/// until one of them exists.
const NO_IDENTITY: &str = "this repository presents no identity to bind.\n\
     - no remote alias: there is no `origin` remote, or its URL names no host (a local path or a \
     file:// URL cannot identify a repository).\n\
     - no root alias: the repository has no commits, or it is a shallow clone (whose oldest \
     commit is a graft boundary, not the real root, so it is deliberately skipped).\n\
     Add an origin remote or make a commit; until one of the two exists this repository cannot \
     be bound.";

// ── Identity ─────────────────────────────────────────────────

fn kind_of(kind: &AliasKind) -> &'static str {
    match kind {
        AliasKind::Remote => "remote",
        AliasKind::Root => "root",
    }
}

/// The aliases of the repository containing the current directory.
pub fn current_repo_aliases() -> Result<Vec<Alias>, LificError> {
    let dir = std::env::current_dir().map_err(|error| {
        LificError::BadRequest(format!("could not read the current directory: {error}"))
    })?;
    repo_aliases(&dir)
}

/// [`current_repo_aliases`] for an explicit directory.
///
/// Errors rather than returning an empty list, because every caller of this
/// command has something to say about a repository with no identity and none
/// of them can proceed without one.
pub fn repo_aliases(dir: &Path) -> Result<Vec<Alias>, LificError> {
    let aliases = repo_identity::compute(dir).map_err(|error| match error {
        RepoIdentityError::NotARepository => LificError::BadRequest(NOT_A_REPOSITORY.into()),
        RepoIdentityError::GitNotFound => LificError::BadRequest(GIT_REQUIRED.into()),
        RepoIdentityError::GitFailed(message) => LificError::Internal(message),
    })?;
    if aliases.is_empty() {
        return Err(LificError::BadRequest(NO_IDENTITY.into()));
    }
    Ok(aliases
        .into_iter()
        .map(|alias| (kind_of(&alias.kind).to_owned(), alias.value))
        .collect())
}

// ── The JSON document ────────────────────────────────────────

fn document(
    resolution: &str,
    project: Value,
    projects: Vec<Value>,
    aliases: &[Alias],
    matched: &HashSet<Alias>,
    created: bool,
) -> Value {
    let aliases: Vec<Value> = aliases
        .iter()
        .map(|pair| {
            json!({
                "kind": pair.0,
                "value": pair.1,
                "matched": matched.contains(pair),
            })
        })
        .collect();
    json!({
        "resolution": resolution,
        "project": project,
        "projects": projects,
        "aliases": aliases,
        "created": created,
    })
}

/// `{ id, identifier, name }` — the only project fields this command reports,
/// matching what `POST /api/repos/resolve` discloses.
pub fn project_summary(project: &Project) -> Value {
    json!({
        "id": project.id,
        "identifier": project.identifier,
        "name": project.name,
    })
}

/// The same three fields, lifted out of a project object the server sent.
pub fn project_summary_from_json(project: &Value) -> Value {
    let field = |key: &str| project.get(key).cloned().unwrap_or(Value::Null);
    json!({
        "id": field("id"),
        "identifier": field("identifier"),
        "name": field("name"),
    })
}

/// The repository resolves to nothing.
pub fn unbound_json(aliases: &[Alias]) -> Value {
    document(
        "none",
        Value::Null,
        Vec::new(),
        aliases,
        &HashSet::new(),
        false,
    )
}

/// The repository resolves to exactly one project — either because it already
/// did, or because this invocation just bound it.
pub fn bound_json(
    project: Value,
    aliases: &[Alias],
    matched: &HashSet<Alias>,
    created: bool,
) -> Value {
    document("one", project, Vec::new(), aliases, matched, created)
}

/// The aliases span several projects. Only an explicit `lific bind PROJECT`
/// can settle it.
pub fn conflict_json(projects: Vec<Value>, aliases: &[Alias]) -> Value {
    document(
        "conflict",
        Value::Null,
        projects,
        aliases,
        &HashSet::new(),
        false,
    )
}

// ── SQL backend ──────────────────────────────────────────────

/// Run `lific bind` against the local database.
pub fn run_sql(
    pool: &DbPool,
    project: Option<&str>,
    create: bool,
    json: bool,
) -> Result<(), LificError> {
    let aliases = current_repo_aliases()?;
    let value = resolve_or_bind(pool, &aliases, project, create)?;
    if json {
        println!(
            "{}",
            crate::cli::term::json_string(&value)
                .map_err(|error| { crate::error::LificError::Internal(error.to_string()) })?
        );
    } else {
        print!(
            "{}",
            crate::cli::ui::sanitize_terminal_block(&human(&value))
        );
    }
    Ok(())
}

/// The whole SQL-side decision, with the identity injected.
///
/// Split from [`run_sql`] so it can be exercised without a git checkout: the
/// aliases are just strings by the time they get here.
pub fn resolve_or_bind(
    pool: &DbPool,
    aliases: &[Alias],
    project: Option<&str>,
    create: bool,
) -> Result<Value, LificError> {
    match project {
        None => report(pool, aliases),
        Some(identifier) => bind(pool, aliases, identifier, create),
    }
}

fn pairs(aliases: &[Alias]) -> Vec<(&str, &str)> {
    aliases
        .iter()
        .map(|(kind, value)| (kind.as_str(), value.as_str()))
        .collect()
}

/// The aliases already sitting on a binding, so the report can say which of
/// the presented ones actually matched.
fn identities_of(conn: &Connection, binding_id: i64) -> Result<HashSet<Alias>, LificError> {
    Ok(repo_bindings::list_identities(conn, binding_id)?
        .into_iter()
        .map(|identity| (identity.kind, identity.value))
        .collect())
}

/// `lific bind` with no project: what does this checkout resolve to?
fn report(pool: &DbPool, aliases: &[Alias]) -> Result<Value, LificError> {
    let conn = pool.read()?;
    match repo_bindings::resolve(&conn, &pairs(aliases))? {
        Resolution::None => Ok(unbound_json(aliases)),
        Resolution::One(binding) => {
            let project = queries::get_project(&conn, binding.project_id)?;
            let matched = identities_of(&conn, binding.id)?;
            Ok(bound_json(
                project_summary(&project),
                aliases,
                &matched,
                false,
            ))
        }
        Resolution::Conflict(bindings) => {
            let mut projects = Vec::with_capacity(bindings.len());
            for binding in &bindings {
                projects.push(project_summary(&queries::get_project(
                    &conn,
                    binding.project_id,
                )?));
            }
            Ok(conflict_json(projects, aliases))
        }
    }
}

/// `lific bind PROJECT`: claim this checkout for that project.
fn bind(
    pool: &DbPool,
    aliases: &[Alias],
    identifier: &str,
    create: bool,
) -> Result<Value, LificError> {
    let pairs = pairs(aliases);
    pool.transaction(|tx| {
        let (project_id, created) = project_for_bind(tx, identifier, create)?;
        let binding = claim_aliases(tx, project_id, &pairs)?;
        let project = queries::get_project(tx, project_id)?;
        let matched = identities_of(tx, binding.id)?;
        Ok(bound_json(
            project_summary(&project),
            aliases,
            &matched,
            created,
        ))
    })
}

/// Resolve the target project, creating it when asked to.
///
/// The identifier doubles as the name. A project has exactly two required
/// fields and one of them is already on the command line, so prompting for
/// the other (or inventing one) would buy nothing that `lific project update
/// --name` cannot fix in a second.
fn project_for_bind(
    conn: &Connection,
    identifier: &str,
    create: bool,
) -> Result<(i64, bool), LificError> {
    match queries::resolve_project_identifier(conn, identifier) {
        Ok(id) => Ok((id, false)),
        Err(LificError::NotFound(_)) if create => {
            let project = queries::create_project(
                conn,
                &CreateProject {
                    name: identifier.to_owned(),
                    identifier: identifier.to_owned(),
                    ..Default::default()
                },
            )?;
            Ok((project.id, true))
        }
        Err(LificError::NotFound(_)) => Err(LificError::NotFound(format!(
            "project '{identifier}' does not exist; pass --create to create it: \
             `lific bind {identifier} --create`"
        ))),
        Err(other) => Err(other),
    }
}

/// Put every presented alias on one binding of `project_id`, and answer with
/// it.
///
/// Idempotent by construction: aliases this project already owns are left
/// alone and only the unclaimed ones are added, so re-running the command
/// returns the same binding untouched. Unlike `POST /api/repos/bind`, a
/// refusal here names the project that owns the alias. The API keeps that
/// secret because an alias is a selector rather than proof of ownership and
/// naming the owner would leak across a visibility boundary; the local
/// operator is already reading the whole database, so withholding it would
/// only make the error useless.
fn claim_aliases(
    conn: &Connection,
    project_id: i64,
    pairs: &[(&str, &str)],
) -> Result<RepoBinding, LificError> {
    let mut ours: BTreeSet<i64> = BTreeSet::new();
    let mut unclaimed: Vec<(&str, &str)> = Vec::new();

    for pair in pairs {
        match repo_bindings::resolve(conn, std::slice::from_ref(pair))? {
            Resolution::None => unclaimed.push(*pair),
            Resolution::One(binding) if binding.project_id == project_id => {
                ours.insert(binding.id);
            }
            Resolution::One(binding) => {
                let owner = queries::get_project(conn, binding.project_id)?;
                return Err(LificError::Conflict(format!(
                    "{} alias '{}' is already bound to project {} ({}); unbind it there first",
                    pair.0, pair.1, owner.identifier, owner.name
                )));
            }
            // `UNIQUE(kind, value)` pins one pair to at most one binding, so
            // this is unreachable. Refuse rather than guess: whatever it is,
            // it is not something this command should quietly extend.
            Resolution::Conflict(_) => {
                return Err(LificError::Conflict(format!(
                    "{} alias '{}' resolves to more than one binding",
                    pair.0, pair.1
                )));
            }
        }
    }

    if ours.len() > 1 {
        return Err(LificError::Conflict(format!(
            "this repository's aliases already sit on {} separate bindings of the same project; \
             merge them before binding again",
            ours.len()
        )));
    }

    match ours.first().copied() {
        Some(binding_id) => {
            for (kind, value) in &unclaimed {
                repo_bindings::add_identity(conn, binding_id, kind, value)?;
            }
            repo_bindings::get(conn, binding_id)?.ok_or_else(|| {
                LificError::Internal(format!("repo binding {binding_id} vanished mid-bind"))
            })
        }
        None => repo_bindings::create_binding(conn, project_id, None, pairs),
    }
}

// ── Human output ─────────────────────────────────────────────

fn alias_lines(out: &mut String, aliases: &[Value], annotate: bool) {
    for alias in aliases {
        let kind = crate::cli::ui::sanitize_terminal_line(alias["kind"].as_str().unwrap_or("?"));
        let value = crate::cli::ui::sanitize_terminal_line(alias["value"].as_str().unwrap_or("?"));
        let note = if !annotate {
            ""
        } else if alias["matched"].as_bool().unwrap_or(false) {
            " (matches this binding)"
        } else {
            " (not on this binding)"
        };
        let _ = writeln!(out, "  {kind:<7} {value}{note}");
    }
}

fn named(project: &Value) -> String {
    let identifier =
        crate::cli::ui::sanitize_terminal_line(project["identifier"].as_str().unwrap_or("?"));
    match project["name"].as_str() {
        Some(name) => {
            let name = crate::cli::ui::sanitize_terminal_line(name);
            if name != identifier {
                format!("{identifier} ({name})")
            } else {
                identifier
            }
        }
        None => identifier,
    }
}

/// Render the JSON document as the text a person reads. Shared by both
/// backends, so `lific bind` looks the same however it reached the data.
#[must_use]
pub fn human(value: &Value) -> String {
    let empty = Vec::new();
    let aliases = value["aliases"].as_array().unwrap_or(&empty);
    let mut out = String::new();

    match value["resolution"].as_str().unwrap_or("none") {
        "one" => {
            if value["created"].as_bool().unwrap_or(false) {
                let _ = writeln!(out, "Created project {}.", named(&value["project"]));
            }
            let _ = writeln!(
                out,
                "This repository is bound to {}.",
                named(&value["project"])
            );
            w(&mut out);
            alias_lines(&mut out, aliases, true);
        }
        "conflict" => {
            let _ = writeln!(
                out,
                "This repository's identity points at more than one project:"
            );
            w(&mut out);
            for project in value["projects"].as_array().unwrap_or(&empty) {
                let _ = writeln!(out, "  {}", named(project));
            }
            w(&mut out);
            alias_lines(&mut out, aliases, false);
            w(&mut out);
            let _ = writeln!(
                out,
                "Two bindings each own one of this repository's aliases. Settle it by merging \
                 them (POST /api/repos/merge) or deleting one (DELETE /api/repos/bindings/{{id}}), \
                 then re-run `lific bind`."
            );
        }
        _ => {
            let _ = writeln!(out, "This repository is not bound to any project.");
            w(&mut out);
            alias_lines(&mut out, aliases, false);
            w(&mut out);
            let _ = writeln!(out, "Bind it with `lific bind PROJECT`.");
        }
    }
    crate::cli::ui::sanitize_terminal_block(&out)
}

fn w(out: &mut String) {
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::path::PathBuf;
    use std::process::Command;

    // ── fixtures ─────────────────────────────────────────────

    fn pool() -> DbPool {
        db::open_memory().expect("test db")
    }

    fn project(pool: &DbPool, identifier: &str, name: &str) -> i64 {
        let conn = pool.write().expect("test writer");
        queries::create_project(
            &conn,
            &CreateProject {
                name: name.into(),
                identifier: identifier.into(),
                ..Default::default()
            },
        )
        .expect("create project")
        .id
    }

    fn alias(kind: &str, value: &str) -> Alias {
        (kind.to_owned(), value.to_owned())
    }

    fn repo_aliases_fixture() -> Vec<Alias> {
        vec![
            alias("remote", "v1:github.com/void/repo"),
            alias("root", "v1:0123456789abcdef0123456789abcdef01234567"),
        ]
    }

    fn bind_to(pool: &DbPool, aliases: &[Alias], identifier: &str, create: bool) -> Value {
        resolve_or_bind(pool, aliases, Some(identifier), create).expect("bind should succeed")
    }

    fn alias_values(value: &Value) -> Vec<(&str, bool)> {
        value["aliases"]
            .as_array()
            .expect("aliases array")
            .iter()
            .map(|alias| {
                (
                    alias["value"].as_str().expect("alias value"),
                    alias["matched"].as_bool().expect("matched flag"),
                )
            })
            .collect()
    }

    // ── binding ──────────────────────────────────────────────

    #[test]
    fn binding_records_every_alias_against_the_project() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        let aliases = repo_aliases_fixture();

        let bound = bind_to(&pool, &aliases, "LIF", false);

        assert_eq!(bound["resolution"], "one");
        assert_eq!(bound["project"]["identifier"], "LIF");
        assert_eq!(bound["project"]["name"], "Lific");
        assert_eq!(bound["created"], false);
        assert_eq!(
            alias_values(&bound),
            vec![
                ("v1:github.com/void/repo", true),
                ("v1:0123456789abcdef0123456789abcdef01234567", true),
            ]
        );
    }

    #[test]
    fn binding_the_same_repository_again_is_idempotent() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        let aliases = repo_aliases_fixture();

        let first = bind_to(&pool, &aliases, "LIF", false);
        let second = bind_to(&pool, &aliases, "LIF", false);

        assert_eq!(first, second);
        let conn = pool.read().unwrap();
        let bindings = repo_bindings::list_for_project(
            &conn,
            queries::resolve_project_identifier(&conn, "LIF").unwrap(),
        )
        .unwrap();
        assert_eq!(bindings.len(), 1, "a rebind must not make a second binding");
    }

    #[test]
    fn a_second_alias_joins_the_binding_the_first_one_made() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        let aliases = repo_aliases_fixture();

        bind_to(&pool, &aliases[..1], "LIF", false);
        let grown = bind_to(&pool, &aliases, "LIF", false);

        let conn = pool.read().unwrap();
        let bindings = repo_bindings::list_for_project(
            &conn,
            queries::resolve_project_identifier(&conn, "LIF").unwrap(),
        )
        .unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            alias_values(&grown),
            vec![
                ("v1:github.com/void/repo", true),
                ("v1:0123456789abcdef0123456789abcdef01234567", true),
            ]
        );
    }

    #[test]
    fn an_alias_owned_by_another_project_names_that_project() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        project(&pool, "OTH", "Other Thing");
        let aliases = repo_aliases_fixture();
        bind_to(&pool, &aliases, "OTH", false);

        let error = resolve_or_bind(&pool, &aliases, Some("LIF"), false)
            .expect_err("an alias another project owns must be refused");

        let message = error.to_string();
        assert!(matches!(error, LificError::Conflict(_)), "got {error:?}");
        assert!(
            message.contains("OTH") && message.contains("Other Thing"),
            "the local refusal must name the owning project: {message}"
        );
    }

    #[test]
    fn an_unknown_project_names_the_create_flag() {
        let pool = pool();
        let aliases = repo_aliases_fixture();

        let error = resolve_or_bind(&pool, &aliases, Some("NEW"), false)
            .expect_err("binding to a project that does not exist must be refused");

        let message = error.to_string();
        assert!(matches!(error, LificError::NotFound(_)), "got {error:?}");
        assert!(
            message.contains("--create"),
            "the refusal must name the flag that fixes it: {message}"
        );
        // Nothing was written on the way to the refusal.
        let conn = pool.read().unwrap();
        assert!(matches!(
            repo_bindings::resolve(&conn, &pairs(&aliases)).unwrap(),
            Resolution::None
        ));
    }

    #[test]
    fn create_makes_the_project_then_binds_to_it() {
        let pool = pool();
        let aliases = repo_aliases_fixture();

        let bound = bind_to(&pool, &aliases, "NEW", true);

        assert_eq!(bound["created"], true);
        assert_eq!(bound["project"]["identifier"], "NEW");
        assert_eq!(
            bound["project"]["name"], "NEW",
            "the identifier doubles as the name"
        );

        // A second bind finds the project and reports it as pre-existing.
        let again = bind_to(&pool, &aliases, "NEW", true);
        assert_eq!(again["created"], false);
    }

    #[test]
    fn creating_a_project_with_an_invalid_identifier_is_refused() {
        let pool = pool();
        let aliases = repo_aliases_fixture();

        let error = resolve_or_bind(&pool, &aliases, Some("lowercase"), true)
            .expect_err("the project identifier rules still apply");

        assert!(matches!(error, LificError::BadRequest(_)), "got {error:?}");
    }

    // ── reporting ────────────────────────────────────────────

    #[test]
    fn reporting_an_unbound_repository_shows_its_aliases() {
        let pool = pool();
        let aliases = repo_aliases_fixture();

        let report = resolve_or_bind(&pool, &aliases, None, false).unwrap();

        assert_eq!(report["resolution"], "none");
        assert_eq!(report["project"], Value::Null);
        assert_eq!(report["created"], false);
        assert_eq!(
            alias_values(&report),
            vec![
                ("v1:github.com/void/repo", false),
                ("v1:0123456789abcdef0123456789abcdef01234567", false),
            ]
        );
        assert!(human(&report).contains("not bound to any project"));
    }

    #[test]
    fn reporting_a_bound_repository_names_the_project_and_the_matching_aliases() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        let aliases = repo_aliases_fixture();
        // Bound through the remote only, so the root alias is presented but
        // does not sit on the binding.
        bind_to(&pool, &aliases[..1], "LIF", false);

        let report = resolve_or_bind(&pool, &aliases, None, false).unwrap();

        assert_eq!(report["resolution"], "one");
        assert_eq!(report["project"]["identifier"], "LIF");
        assert_eq!(
            alias_values(&report),
            vec![
                ("v1:github.com/void/repo", true),
                ("v1:0123456789abcdef0123456789abcdef01234567", false),
            ]
        );
        let text = human(&report);
        assert!(text.contains("LIF (Lific)"), "{text}");
        assert!(text.contains("(matches this binding)"), "{text}");
        assert!(text.contains("(not on this binding)"), "{text}");
    }

    #[test]
    fn reporting_a_conflicted_repository_lists_every_project() {
        let pool = pool();
        project(&pool, "LIF", "Lific");
        project(&pool, "OTH", "Other Thing");
        let aliases = repo_aliases_fixture();
        bind_to(&pool, &aliases[..1], "LIF", false);
        bind_to(&pool, &aliases[1..], "OTH", false);

        let report = resolve_or_bind(&pool, &aliases, None, false).unwrap();

        assert_eq!(report["resolution"], "conflict");
        assert_eq!(report["project"], Value::Null);
        let identifiers: Vec<&str> = report["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|project| project["identifier"].as_str().unwrap())
            .collect();
        assert_eq!(identifiers, vec!["LIF", "OTH"]);
        let text = human(&report);
        assert!(text.contains("more than one project"), "{text}");
        // The guidance must name real remedies. `lific bind PROJECT` is not
        // one: claim_aliases refuses aliases owned by another project, so a
        // conflict can only be settled by merging or deleting a binding.
        assert!(text.contains("/api/repos/merge"), "{text}");
        assert!(text.contains("re-run `lific bind`"), "{text}");
    }

    #[test]
    fn hostile_stored_bind_names_are_inert_in_human_output() {
        let pool = pool();
        let name = "Lific\x1b]52;c;cHdu\x07\x1b[2J\u{009b}\u{202e}\u{2066}\u{200b}\u{feff}\r\n\t\u{0008}\u{007f}";
        project(&pool, "LIF", name);
        project(&pool, "OTH", name);
        let aliases = repo_aliases_fixture();
        bind_to(&pool, &aliases[..1], "LIF", false);
        let bound = resolve_or_bind(&pool, &aliases, None, false).unwrap();
        bind_to(&pool, &aliases[1..], "OTH", false);
        let conflict = resolve_or_bind(&pool, &aliases, None, false).unwrap();

        assert_eq!(bound["project"]["name"], name);
        assert_eq!(conflict["projects"][0]["name"], name);
        for (value, rows) in [(bound, 4), (conflict, 9)] {
            let text = human(&value);
            for control in [
                '\x1b', '\x07', '\u{009b}', '\u{202e}', '\u{2066}', '\u{200b}', '\u{feff}', '\r',
                '\t', '\u{0008}', '\u{007f}',
            ] {
                assert!(!text.contains(control), "unsafe output: {text:?}");
            }
            assert_eq!(
                text.lines().count(),
                rows,
                "name must not create a new row: {text:?}"
            );
            let encoded = crate::cli::term::json_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
        }
    }

    #[test]
    fn hostile_bind_response_fields_cannot_add_rows() {
        let aliases = vec![alias("remote\n\t\x1b", "v1:host/repo\r\n\u{009b}\u{202e}")];
        let project = json!({"identifier": "LIF\n\x1b", "name": "Name\n\u{202e}"});
        for (value, rows) in [
            (unbound_json(&aliases), 5),
            (
                bound_json(project.clone(), &aliases, &HashSet::new(), true),
                4,
            ),
            (conflict_json(vec![project], &aliases), 7),
        ] {
            let text = human(&value);
            for control in ['\x1b', '\u{009b}', '\u{202e}', '\r', '\t'] {
                assert!(!text.contains(control), "unsafe output: {text:?}");
            }
            assert_eq!(
                text.lines().count(),
                rows,
                "fields must not add rows: {text:?}"
            );
            assert!(
                text.lines()
                    .any(|line| line.contains("remote") && line.contains("v1:host/repo")),
                "{text:?}"
            );
            let encoded = crate::cli::term::json_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
        }
    }

    // ── identity, against real repositories ──────────────────

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git should be installed for these tests");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create repo dir");
        git(dir, &["init", "-q", "-b", "main"]);
    }

    fn commit(dir: &Path) {
        git(
            dir,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
    }

    fn sub(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    #[test]
    fn a_repository_with_no_origin_and_no_commits_explains_both_missing_aliases() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = sub(&tmp, "empty");
        init_repo(&repo);

        let error = repo_aliases(&repo).expect_err("an identity-less repo cannot be bound");

        let message = error.to_string();
        assert!(matches!(error, LificError::BadRequest(_)), "got {error:?}");
        assert!(message.contains("no remote alias"), "{message}");
        assert!(message.contains("origin"), "{message}");
        assert!(message.contains("no root alias"), "{message}");
        assert!(message.contains("no commits"), "{message}");
        assert!(message.contains("shallow"), "{message}");
    }

    #[test]
    fn a_directory_outside_a_repository_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = sub(&tmp, "plain");
        std::fs::create_dir_all(&dir).unwrap();

        let error = repo_aliases(&dir).expect_err("a non-repository cannot be bound");

        assert!(
            error.to_string().contains("not inside a git repository"),
            "{error}"
        );
    }

    #[test]
    fn a_real_checkout_binds_and_then_resolves_to_its_project() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo);
        git(
            &repo,
            &["remote", "add", "origin", "git@github.com:void/real.git"],
        );

        let aliases = repo_aliases(&repo).expect("a repo with an origin and a commit has aliases");
        assert_eq!(aliases[0].0, "remote");
        assert_eq!(aliases[0].1, "v1:github.com/void/real");
        assert_eq!(aliases[1].0, "root");

        let pool = pool();
        let bound = bind_to(&pool, &aliases, "REAL", true);
        assert_eq!(bound["created"], true);

        let report = resolve_or_bind(&pool, &aliases, None, false).unwrap();
        assert_eq!(report["resolution"], "one");
        assert_eq!(report["project"]["identifier"], "REAL");
        assert!(
            alias_values(&report).iter().all(|(_, matched)| *matched),
            "both aliases of the checkout must resolve to the binding"
        );
    }
}
