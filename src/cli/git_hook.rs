//! `lific git-hook` — close the issues a commit message says it closes
//! (LIF-5).
//!
//! The grammar is [`crate::issue_refs`], shared with `POST /api/git-hook` so a
//! local hook and a CI pipeline can never disagree about what "closes LIF-42"
//! means.
//!
//! Two ways in, matching the two shapes a git hook takes:
//!
//! * No `--range`: stdin is read to EOF as a single message. This is the
//!   `commit-msg` hook, whose argument is a file containing the message
//!   (`lific git-hook < "$1"`), and equally anything piped in.
//! * `--range A..B`: the messages come from `git log` over that range. This is
//!   the `post-receive` hook and the CI step, both of which learn about a
//!   batch of commits at once.
//!
//! Both backends emit the same JSON document, so `--json` output does not
//! depend on the transport, and [`human`] renders that document for both.
//!
//! # JSON shape
//!
//! ```json
//! { "closed": ["LIF-42"], "skipped": [ { "identifier": "LIF-9", "reason": "not found" } ] }
//! ```
//!
//! Under `--dry-run` the first key is `would_close` instead, so a rehearsal
//! can never be mistaken for a write by something reading the output.
//!
//! # The SQL backend has no authorization to apply
//!
//! Running against a local database means holding the database file, which is
//! the whole of the local trust model: `lific issue update --status done` does
//! not consult project roles either. The skip reasons the SQL path can produce
//! are therefore "not found" and "already closed" only. The HTTP path talks to
//! the endpoint, which gates every identifier at Maintainer and can also skip
//! with "forbidden".

use std::fmt::Write as _;
use std::io::Read as _;
use std::process::Command;

use serde_json::{Value, json};

use crate::db::DbPool;
use crate::db::models::{AttachmentActor, Status, UpdateIssue};
use crate::db::queries;
use crate::error::LificError;
use crate::issue_refs;

/// The record separator `git log --format=%B%x1e` puts after every message.
/// A commit message can contain anything printable, including blank lines, so
/// the delimiter has to be something a human will never type.
const RECORD_SEPARATOR: char = '\u{1e}';

pub(crate) const NOT_FOUND: &str = "not found";
pub(crate) const ALREADY_CLOSED: &str = "already closed";

// ── Input ────────────────────────────────────────────────────

/// The commit messages this invocation should scan.
pub fn messages(range: Option<&str>) -> Result<Vec<String>, LificError> {
    match range {
        Some(range) => messages_from_range(range),
        None => {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer).map_err(|e| {
                LificError::BadRequest(format!("could not read the commit message from stdin: {e}"))
            })?;
            Ok(vec![buffer])
        }
    }
}

/// `git log` over a range, one message per record.
///
/// Run through `std::process::Command` with the range as a single argument, so
/// no shell ever sees it. A range git rejects surfaces git's own stderr,
/// because git explains a bad revision far better than a wrapper can.
fn messages_from_range(range: &str) -> Result<Vec<String>, LificError> {
    let output = Command::new("git")
        .args(["log", "--format=%B%x1e", range])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LificError::BadRequest(
                    "`lific git-hook --range` reads commit messages by running git, and the `git` \
                     binary was not found on PATH"
                        .into(),
                )
            } else {
                LificError::BadRequest(format!("could not run git: {e}"))
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(LificError::BadRequest(if stderr.is_empty() {
            format!("git log {range} failed")
        } else {
            stderr
        }));
    }

    Ok(split_records(&String::from_utf8_lossy(&output.stdout)))
}

/// Split `git log`'s output on the record separator, dropping the empty tail
/// the trailing separator leaves behind.
fn split_records(stdout: &str) -> Vec<String> {
    stdout
        .split(RECORD_SEPARATOR)
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .map(str::to_owned)
        .collect()
}

// ── The JSON document ────────────────────────────────────────

fn skipped(identifier: &str, reason: &str) -> Value {
    json!({ "identifier": identifier, "reason": reason })
}

pub(crate) fn document(dry_run: bool, acted: Vec<String>, skipped: Vec<Value>) -> Value {
    let key = if dry_run { "would_close" } else { "closed" };
    json!({ key: acted, "skipped": skipped })
}

// ── SQL backend ──────────────────────────────────────────────

/// Run `lific git-hook` against the local database.
pub fn run_sql(
    pool: &DbPool,
    range: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<(), LificError> {
    let messages = messages(range)?;
    let value = close_referenced(pool, &messages, dry_run)?;
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

/// The whole SQL-side decision, with the messages injected — so it can be
/// exercised without a git checkout or a pipe underneath the test process.
///
/// Each identifier is closed through [`queries::update_issue`], the same
/// function `lific issue update --status done` calls, so the status
/// transition, the audit trail and the `seq` stamp all land exactly as they
/// would for a hand-typed close.
pub fn close_referenced<S: AsRef<str>>(
    pool: &DbPool,
    messages: &[S],
    dry_run: bool,
) -> Result<Value, LificError> {
    let mut acted = Vec::new();
    let mut skips = Vec::new();

    for identifier in issue_refs::closing_references_in(messages) {
        let conn = pool.write()?;
        let issue = match queries::resolve_identifier(&conn, &identifier)
            .and_then(|id| queries::get_issue(&conn, id))
        {
            Ok(issue) => issue,
            Err(LificError::NotFound(_) | LificError::BadRequest(_)) => {
                skips.push(skipped(&identifier, NOT_FOUND));
                continue;
            }
            Err(other) => return Err(other),
        };

        if issue.status.is_closed() {
            skips.push(skipped(&identifier, ALREADY_CLOSED));
            continue;
        }
        if !dry_run {
            queries::update_issue(
                &conn,
                issue.id,
                &UpdateIssue {
                    status: Some(Status::Done),
                    // LIF-409: closing rewrites nothing, but it re-scans the
                    // description exactly as `issue update` does, so this
                    // backend's git hook stays indistinguishable from a
                    // hand-typed close. A direct-SQL caller is the trusted
                    // local operator.
                    attachments: AttachmentActor::TrustedLocal,
                    ..Default::default()
                },
            )?;
        }
        acted.push(identifier);
    }

    Ok(document(dry_run, acted, skips))
}

// ── Human output ─────────────────────────────────────────────

/// Render the JSON document as the text a person reads. Shared by both
/// backends, so `lific git-hook` looks the same however it reached the data.
#[must_use]
pub fn human(value: &Value) -> String {
    let empty = Vec::new();
    let dry_run = value.get("would_close").is_some();
    let acted = value
        .get(if dry_run { "would_close" } else { "closed" })
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let skipped = value["skipped"].as_array().unwrap_or(&empty);

    let mut out = String::new();
    for identifier in acted {
        let identifier = crate::cli::ui::sanitize_terminal_line(identifier.as_str().unwrap_or("?"));
        let verb = if dry_run { "would close" } else { "closed" };
        let _ = writeln!(out, "  {verb} {identifier}");
    }
    for skip in skipped {
        let identifier =
            crate::cli::ui::sanitize_terminal_line(skip["identifier"].as_str().unwrap_or("?"));
        let reason = crate::cli::ui::sanitize_terminal_line(skip["reason"].as_str().unwrap_or("?"));
        let _ = writeln!(out, "  skipped {identifier} ({reason})");
    }

    if acted.is_empty() && skipped.is_empty() {
        out.push_str("No issue references found in these commit messages.\n");
        return crate::cli::ui::sanitize_terminal_block(&out);
    }

    let _ = writeln!(
        out,
        "\n{} {}, {} skipped.",
        acted.len(),
        if dry_run { "to close" } else { "closed" },
        skipped.len()
    );
    crate::cli::ui::sanitize_terminal_block(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::CreateProject;
    use std::path::Path;

    // ── fixtures ─────────────────────────────────────────────

    fn pool() -> DbPool {
        db::open_memory().expect("test db")
    }

    /// A project with `count` issues in it, all `backlog`.
    fn seed(pool: &DbPool, identifier: &str, count: usize) -> Vec<i64> {
        let conn = pool.write().expect("test writer");
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: format!("Project {identifier}"),
                identifier: identifier.into(),
                ..Default::default()
            },
        )
        .expect("create project");
        (0..count)
            .map(|n| {
                queries::create_issue(
                    &conn,
                    &crate::db::models::CreateIssue {
                        project_id: project.id,
                        title: format!("Issue {n}"),
                        ..Default::default()
                    },
                )
                .expect("create issue")
                .id
            })
            .collect()
    }

    fn status_of(pool: &DbPool, id: i64) -> Status {
        let conn = pool.read().expect("test reader");
        queries::get_issue(&conn, id).expect("read issue").status
    }

    fn identifiers(value: &Value, key: &str) -> Vec<String> {
        value[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|entry| entry.as_str().expect("string").to_owned())
            .collect()
    }

    // ── closing ──────────────────────────────────────────────

    #[test]
    fn a_closing_message_closes_the_issue() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 1);

        let value = close_referenced(&pool, &["Do the thing\n\nCloses LIF-1"], false).unwrap();

        assert_eq!(identifiers(&value, "closed"), vec!["LIF-1"]);
        assert_eq!(value["skipped"].as_array().unwrap().len(), 0);
        assert_eq!(status_of(&pool, ids[0]), Status::Done);
    }

    #[test]
    fn several_messages_close_several_issues_and_deduplicate() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 2);

        let value = close_referenced(
            &pool,
            &["Closes LIF-1", "fixes LIF-2 and closes lif-1"],
            false,
        )
        .unwrap();

        assert_eq!(identifiers(&value, "closed"), vec!["LIF-1", "LIF-2"]);
        assert_eq!(status_of(&pool, ids[0]), Status::Done);
        assert_eq!(status_of(&pool, ids[1]), Status::Done);
    }

    #[test]
    fn a_dry_run_reports_what_would_close_and_writes_nothing() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 1);

        let value = close_referenced(&pool, &["closes LIF-1"], true).unwrap();

        assert_eq!(identifiers(&value, "would_close"), vec!["LIF-1"]);
        assert!(value.get("closed").is_none());
        assert_eq!(
            status_of(&pool, ids[0]),
            Status::Backlog,
            "a rehearsal must not touch the database"
        );
    }

    #[test]
    fn an_unknown_identifier_is_skipped_as_not_found() {
        let pool = pool();
        seed(&pool, "LIF", 1);

        let value = close_referenced(&pool, &["closes LIF-99, closes NOPE-1"], false).unwrap();

        assert_eq!(identifiers(&value, "closed"), Vec::<String>::new());
        assert_eq!(
            value["skipped"],
            json!([
                {"identifier": "LIF-99", "reason": "not found"},
                {"identifier": "NOPE-1", "reason": "not found"},
            ])
        );
    }

    #[test]
    fn an_already_closed_issue_is_skipped() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 2);
        close_referenced(&pool, &["closes LIF-1"], false).unwrap();
        {
            let conn = pool.write().unwrap();
            queries::update_issue(
                &conn,
                ids[1],
                &UpdateIssue {
                    status: Some(Status::Cancelled),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let value = close_referenced(&pool, &["closes LIF-1, fixes LIF-2"], false).unwrap();

        assert_eq!(
            value["skipped"],
            json!([
                {"identifier": "LIF-1", "reason": "already closed"},
                {"identifier": "LIF-2", "reason": "already closed"},
            ]),
            "both terminal statuses read as already closed"
        );
    }

    #[test]
    fn a_message_that_only_mentions_an_issue_changes_nothing() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 1);

        let value = close_referenced(&pool, &["Refactor the scanner (see LIF-1)"], false).unwrap();

        assert_eq!(value, json!({"closed": [], "skipped": []}));
        assert_eq!(status_of(&pool, ids[0]), Status::Backlog);
        assert!(human(&value).contains("No issue references found"));
    }

    #[test]
    fn closing_goes_through_the_ordinary_update_path() {
        let pool = pool();
        let ids = seed(&pool, "LIF", 1);
        close_referenced(&pool, &["closes LIF-1"], false).unwrap();

        let conn = pool.read().unwrap();
        let transitions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM status_transitions WHERE issue_id = ?1",
                [ids[0]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            transitions, 1,
            "the close must be recorded like any other status change"
        );
    }

    // ── rendering ────────────────────────────────────────────

    #[test]
    fn human_output_names_every_issue_and_summarizes() {
        let value = document(
            false,
            vec!["LIF-1".into()],
            vec![skipped("LIF-2", NOT_FOUND)],
        );
        let text = human(&value);
        assert!(text.contains("closed LIF-1"), "{text}");
        assert!(text.contains("skipped LIF-2 (not found)"), "{text}");
        assert!(text.contains("1 closed, 1 skipped."), "{text}");
    }

    #[test]
    fn human_output_of_a_dry_run_says_would_close() {
        let text = human(&document(true, vec!["LIF-1".into()], Vec::new()));
        assert!(text.contains("would close LIF-1"), "{text}");
        assert!(text.contains("1 to close, 0 skipped."), "{text}");
    }

    #[test]
    fn hostile_git_hook_http_fields_are_inert_in_human_output() {
        let hostile = "\x1b]52;c;cHdu\x07\x1b[2J\u{009b}\u{202e}\u{2066}\u{200b}\u{feff}\r\n\t\u{0008}\u{007f}";
        for key in ["closed", "would_close"] {
            let value = json!({
                key: [format!("LIF-1{hostile}")],
                "skipped": [{
                    "identifier": format!("LIF-2{hostile}"),
                    "reason": format!("denied{hostile}")
                }]
            });
            let text = human(&value);
            for control in [
                '\x1b', '\x07', '\u{009b}', '\u{202e}', '\u{2066}', '\u{200b}', '\u{feff}', '\r',
                '\t', '\u{0008}', '\u{007f}',
            ] {
                assert!(!text.contains(control), "unsafe output: {text:?}");
            }
            assert_eq!(
                text.lines().count(),
                4,
                "fields must not add rows: {text:?}"
            );
            assert!(text.lines().nth(1).unwrap().contains("denied"));
            let encoded = crate::cli::term::json_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
        }
    }

    // ── ranges, against a real repository ────────────────────

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

    fn commit(dir: &Path, message: &str) {
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
                message,
            ],
        );
    }

    /// A repository with two commits, each closing a different issue.
    fn two_commit_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create repo dir");
        git(dir, &["init", "-q", "-b", "main"]);
        commit(dir, "Base commit");
        commit(dir, "Add the thing\n\nCloses LIF-1");
        commit(dir, "Fix the other thing\n\nFixes LIF-2");
    }

    /// `messages_from_range` with the working directory pinned, since
    /// `git log` has no `-C` equivalent through this helper's argument list.
    fn range_in(dir: &Path, range: &str) -> Result<Vec<String>, LificError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["log", "--format=%B%x1e", range])
            .output()
            .expect("git runs");
        if !output.status.success() {
            return Err(LificError::BadRequest(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        Ok(split_records(&String::from_utf8_lossy(&output.stdout)))
    }

    #[test]
    fn a_range_yields_one_message_per_commit_and_closes_them_all() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        two_commit_repo(&repo);

        let messages = range_in(&repo, "HEAD~2..HEAD").expect("a valid range");
        assert_eq!(messages.len(), 2, "one record per commit: {messages:?}");

        let pool = pool();
        let ids = seed(&pool, "LIF", 2);
        let value = close_referenced(&pool, &messages, false).unwrap();

        // `git log` is newest-first, so the second commit's issue comes first.
        assert_eq!(identifiers(&value, "closed"), vec!["LIF-2", "LIF-1"]);
        assert_eq!(status_of(&pool, ids[0]), Status::Done);
        assert_eq!(status_of(&pool, ids[1]), Status::Done);
    }

    #[test]
    fn a_bad_range_surfaces_gits_own_complaint() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        two_commit_repo(&repo);

        let error = range_in(&repo, "no-such-ref..HEAD").expect_err("git rejects the range");

        let message = error.to_string();
        assert!(matches!(error, LificError::BadRequest(_)), "got {error:?}");
        assert!(
            message.contains("no-such-ref"),
            "the refusal must carry git's own words: {message}"
        );
    }

    #[test]
    fn the_trailing_record_separator_does_not_produce_an_empty_message() {
        assert_eq!(
            split_records("first\u{1e}\nsecond\u{1e}\n"),
            vec!["first".to_owned(), "second".to_owned()]
        );
        assert!(split_records("").is_empty());
    }
}
