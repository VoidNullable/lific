//! Repository identity: derive stable aliases for a git repository on disk.
//!
//! This is a pure local computation. It shells out to `git` (never a shell) in
//! the given directory and returns zero, one, or two aliases:
//!
//! - [`AliasKind::Remote`] — the normalized canonical URL of the `origin` remote.
//! - [`AliasKind::Root`] — the root commit of HEAD's first-parent chain.
//!
//! Zero aliases means the repository is unbindable (no origin, no commits); it
//! is not an error, and the caller is responsible for explaining why.
//!
//! Nothing here touches the database, the network, or server state.

use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

/// Which property of the repository an alias was derived from.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasKind {
    /// Normalized `origin` remote URL.
    Remote,
    /// Root commit of HEAD's first-parent chain.
    Root,
}

/// A single repository identity alias. `value` carries the `v1:` prefix.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub kind: AliasKind,
    pub value: String,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, thiserror::Error)]
pub enum RepoIdentityError {
    /// The `git` binary is not on PATH.
    #[error("git binary not found on PATH")]
    GitNotFound,
    /// The directory is not inside a git repository.
    #[error("directory is not inside a git repository")]
    NotARepository,
    /// git ran but misbehaved. The message names the command.
    #[error("{0}")]
    GitFailed(String),
}

/// Version prefix on every alias value.
const ALIAS_PREFIX: &str = "v1:";

/// Upper bound on stdout we are willing to interpret from a git invocation.
/// A single OID line is ~65 bytes; a remote URL is short. Anything larger is a
/// misbehaving git, not data.
const MAX_GIT_STDOUT: usize = 4096;

/// Environment variables that redirect git's repository discovery or rewrite
/// the history graph it reports. Cleared on every invocation so the answer
/// depends only on the directory we were handed.
const GIT_ENV_TO_CLEAR: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
    "GIT_CEILING_DIRECTORIES",
];

/// Compute the identity aliases for the repository containing `dir`.
///
/// Returns 0, 1, or 2 aliases. Zero means the repository is unbindable.
#[cfg_attr(not(test), allow(dead_code))]
pub fn compute(dir: &Path) -> Result<Vec<Alias>, RepoIdentityError> {
    compute_with_git("git", dir)
}

/// [`compute`], with the git program name injected so tests can exercise the
/// missing-binary path.
#[cfg_attr(not(test), allow(dead_code))]
pub fn compute_with_git(git: &str, dir: &Path) -> Result<Vec<Alias>, RepoIdentityError> {
    // Establish that we are in a repository at all. Using git's own discovery
    // means submodules and linked worktrees resolve to their own repository,
    // exactly as any other git command in that directory would.
    let probe = run_git(git, dir, &[], &["rev-parse", "--git-dir"])?;
    if !probe.status.success() {
        return Err(RepoIdentityError::NotARepository);
    }

    let mut aliases = Vec::new();

    if let Some(remote) = remote_alias(git, dir)? {
        aliases.push(remote);
    }
    if let Some(root) = root_alias(git, dir)? {
        aliases.push(root);
    }

    Ok(aliases)
}

/// The normalized `origin` remote, if there is one and it names a host.
fn remote_alias(git: &str, dir: &Path) -> Result<Option<Alias>, RepoIdentityError> {
    let output = run_git(git, dir, &[], &["remote", "get-url", "origin"])?;
    // A missing `origin` exits non-zero. That is a repository without a remote
    // alias, not a failure.
    if !output.status.success() || output.stdout.len() > MAX_GIT_STDOUT {
        return Ok(None);
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    Ok(normalize_remote_url(&raw).map(|value| Alias {
        kind: AliasKind::Remote,
        value,
    }))
}

/// The root of HEAD's first-parent chain, if it can be determined honestly.
fn root_alias(git: &str, dir: &Path) -> Result<Option<Alias>, RepoIdentityError> {
    // A shallow clone reports its graft boundary as parentless, which would be
    // silently wrong rather than loudly missing. Skip the alias entirely.
    let shallow = run_git(git, dir, &[], &["rev-parse", "--is-shallow-repository"])?;
    if shallow.status.success() && String::from_utf8_lossy(&shallow.stdout).trim() == "true" {
        return Ok(None);
    }

    // `--first-parent` is load-bearing. `--all` depends on which refs a clone
    // happened to fetch (an orphan gh-pages branch changes the answer), and
    // "oldest by timestamp" changes retroactively when unrelated history is
    // merged in. Neither is stable; the first-parent root is.
    let args = ["rev-list", "--first-parent", "--max-parents=0", "HEAD"];
    let output = run_git(git, dir, &["--no-replace-objects"], &args)?;
    // An empty repository has no HEAD commit and rev-list exits non-zero. That
    // is a repository without a root alias, not a failure.
    if !output.status.success() {
        return Ok(None);
    }
    if output.stdout.len() > MAX_GIT_STDOUT {
        return Err(git_failed(
            dir,
            &args,
            "produced more output than a single OID",
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    let mut lines = stdout.lines();
    let (Some(oid), None) = (lines.next(), lines.next()) else {
        return Err(git_failed(
            dir,
            &args,
            "did not produce exactly one commit id",
        ));
    };
    if !is_hex_oid(oid) {
        return Err(git_failed(dir, &args, "produced a malformed commit id"));
    }

    Ok(Some(Alias {
        kind: AliasKind::Root,
        value: format!("{ALIAS_PREFIX}{oid}"),
    }))
}

/// SHA-1 (40) or SHA-256 (64) object id, lowercase or uppercase hex.
fn is_hex_oid(candidate: &str) -> bool {
    matches!(candidate.len(), 40 | 64) && candidate.bytes().all(|b| b.is_ascii_hexdigit())
}

fn git_failed(dir: &Path, args: &[&str], what: &str) -> RepoIdentityError {
    RepoIdentityError::GitFailed(format!(
        "`git {}` in {} {what}",
        args.join(" "),
        dir.display()
    ))
}

/// Run git with a scrubbed environment. Never goes through a shell.
fn run_git(
    git: &str,
    dir: &Path,
    globals: &[&str],
    args: &[&str],
) -> Result<Output, RepoIdentityError> {
    let mut command = Command::new(git);
    for var in GIT_ENV_TO_CLEAR {
        command.env_remove(var);
    }
    command.args(globals);
    command.arg("-C").arg(dir);
    command.args(args);

    command.output().map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            RepoIdentityError::GitNotFound
        } else {
            git_failed(dir, args, &format!("could not be run: {err}"))
        }
    })
}

/// Normalize a git remote URL to `v1:{host}/{path}`.
///
/// Returns `None` for anything that does not name a host: `file://` URLs, bare
/// filesystem paths, and malformed input. Host is lowercased; an explicit port
/// is kept; IPv6 literals stay bracketed. Path case is preserved because
/// case-sensitive forges exist, and percent-encoding is left untouched because
/// decoding it would introduce ambiguity.
fn normalize_remote_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let (host_port, path) = match split_scheme(raw) {
        Some((scheme, rest)) => {
            if !matches!(scheme.as_str(), "ssh" | "https" | "http" | "git") {
                return None;
            }
            let (authority, path) = match rest.find('/') {
                Some(index) => (&rest[..index], &rest[index..]),
                None => (rest, ""),
            };
            (parse_host_port(strip_credentials(authority))?, path)
        }
        None => parse_scp_like(raw)?,
    };

    let path = path.trim_matches('/');
    // Strip exactly one trailing `.git`.
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_matches('/');
    if path.is_empty() {
        return None;
    }

    Some(format!("{ALIAS_PREFIX}{host_port}/{path}"))
}

/// Split `scheme://rest`, rejecting anything whose scheme is not a plausible
/// URL scheme (which is how scp-form addresses fall through to their parser).
fn split_scheme(raw: &str) -> Option<(String, &str)> {
    let (scheme, rest) = raw.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let mut chars = scheme.chars();
    let first_is_alpha = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let tail_is_valid = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !first_is_alpha || !tail_is_valid {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), rest))
}

/// Drop `user@` or `user:password@` from an authority.
fn strip_credentials(authority: &str) -> &str {
    match authority.rfind('@') {
        Some(index) => &authority[index + 1..],
        None => authority,
    }
}

/// Parse `host`, `host:port`, `[::1]`, or `[::1]:port` into its canonical form.
fn parse_host_port(authority: &str) -> Option<String> {
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host = &authority[..=close];
        // Reject `[]`.
        if close < 2 {
            return None;
        }
        let rest = &authority[close + 1..];
        let port = if rest.is_empty() {
            None
        } else {
            Some(rest.strip_prefix(':')?)
        };
        (host, port)
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };

    if host.is_empty() || host.contains(|c: char| c.is_whitespace()) {
        return None;
    }
    let host = host.to_ascii_lowercase();

    match port {
        // `host:` — an empty port is just no port.
        None | Some("") => Some(host),
        Some(port) if port.bytes().all(|b| b.is_ascii_digit()) => Some(format!("{host}:{port}")),
        // A non-numeric port means this was never a URL we understand.
        Some(_) => None,
    }
}

/// Parse scp-form `[user@]host:path`, including `[user@][::1]:path`.
///
/// The separating colon must come before any slash; that is what distinguishes
/// `git@host:a/b` from the bare filesystem path `/tmp/x`.
fn parse_scp_like(raw: &str) -> Option<(String, &str)> {
    let head_end = raw.find('/').unwrap_or(raw.len());
    if !raw[..head_end].contains(':') {
        return None;
    }
    let rest = match raw[..head_end].rfind('@') {
        Some(index) => &raw[index + 1..],
        None => raw,
    };

    let (host, path) = if rest.starts_with('[') {
        let close = rest.find(']')?;
        if close < 2 {
            return None;
        }
        (&rest[..=close], rest[close + 1..].strip_prefix(':')?)
    } else {
        rest.split_once(':')?
    };

    if host.is_empty() || host.contains(|c: char| c.is_whitespace()) {
        return None;
    }
    Some((host.to_ascii_lowercase(), path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    // --- test helpers ---------------------------------------------------

    fn git_raw(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git should be installed for these tests")
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let output = git_raw(dir, args);
        assert!(
            output.status.success(),
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create repo dir");
        git_ok(dir, &["init", "-q", "-b", "main"]);
    }

    fn commit(dir: &Path, message: &str) {
        git_ok(
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

    fn orphan_commit(dir: &Path, branch: &str, message: &str) {
        git_ok(dir, &["checkout", "-q", "--orphan", branch]);
        commit(dir, message);
    }

    fn aliases_of(dir: &Path) -> Vec<Alias> {
        compute(dir).expect("compute should succeed on a real repository")
    }

    fn kinds(aliases: &[Alias]) -> Vec<AliasKind> {
        aliases.iter().map(|a| a.kind.clone()).collect()
    }

    fn value_of(aliases: &[Alias], kind: &AliasKind) -> Option<String> {
        aliases
            .iter()
            .find(|a| &a.kind == kind)
            .map(|a| a.value.clone())
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }

    fn sub(dir: &tempfile::TempDir, name: &str) -> PathBuf {
        dir.path().join(name)
    }

    // --- normalization: equivalent forms --------------------------------

    #[test]
    fn ssh_scp_and_https_forms_of_the_same_repo_normalize_identically() {
        let expected = "v1:github.com/VoidNullable/lific";
        for url in [
            "ssh://git@github.com/VoidNullable/lific.git",
            "git@github.com:VoidNullable/lific.git",
            "git@github.com:VoidNullable/lific",
            "https://github.com/VoidNullable/lific",
            "https://github.com/VoidNullable/lific.git",
            "git://github.com/VoidNullable/lific.git",
            "https://github.com/VoidNullable/lific/",
        ] {
            assert_eq!(
                normalize_remote_url(url).as_deref(),
                Some(expected),
                "url: {url}"
            );
        }
    }

    #[test]
    fn only_one_trailing_dot_git_suffix_is_stripped() {
        assert_eq!(
            normalize_remote_url("https://host/a/b.git.git").as_deref(),
            Some("v1:host/a/b.git")
        );
    }

    #[test]
    fn credentials_are_stripped_from_the_authority() {
        assert_eq!(
            normalize_remote_url("https://user:pass@host/a/b").as_deref(),
            Some("v1:host/a/b")
        );
        assert_eq!(
            normalize_remote_url("https://user@host/a/b").as_deref(),
            Some("v1:host/a/b")
        );
    }

    #[test]
    fn an_explicit_port_is_kept() {
        assert_eq!(
            normalize_remote_url("ssh://git@host:2222/a/b.git").as_deref(),
            Some("v1:host:2222/a/b")
        );
        assert_eq!(
            normalize_remote_url("https://host:8443/a/b").as_deref(),
            Some("v1:host:8443/a/b")
        );
    }

    #[test]
    fn host_is_lowercased_but_path_case_is_preserved() {
        assert_eq!(
            normalize_remote_url("git@GitHub.com:VoidNullable/lific.git").as_deref(),
            Some("v1:github.com/VoidNullable/lific")
        );
        assert_eq!(
            normalize_remote_url("https://GITHUB.COM/VoidNullable/Lific").as_deref(),
            Some("v1:github.com/VoidNullable/Lific")
        );
    }

    #[test]
    fn ipv6_literals_keep_their_brackets() {
        assert_eq!(
            normalize_remote_url("ssh://git@[::1]:2222/a/b.git").as_deref(),
            Some("v1:[::1]:2222/a/b")
        );
        assert_eq!(
            normalize_remote_url("ssh://[2001:DB8::1]/a/b").as_deref(),
            Some("v1:[2001:db8::1]/a/b")
        );
        assert_eq!(
            normalize_remote_url("git@[::1]:a/b.git").as_deref(),
            Some("v1:[::1]/a/b")
        );
    }

    #[test]
    fn percent_encoding_in_the_path_is_left_untouched() {
        assert_eq!(
            normalize_remote_url("https://host/a%2Fb/c").as_deref(),
            Some("v1:host/a%2Fb/c")
        );
    }

    // --- normalization: rejections --------------------------------------

    #[test]
    fn local_urls_and_bare_paths_are_rejected() {
        for url in [
            "file:///tmp/x",
            "/tmp/x",
            "./x",
            "../x",
            "relative/path",
            "",
            "   ",
            "https://",
            "https:///a/b",
            "https://host",
            "https://host/",
            "ftp://host/a/b",
        ] {
            assert_eq!(normalize_remote_url(url), None, "url: {url}");
        }
    }

    // --- repository behavior --------------------------------------------

    #[test]
    fn a_repo_with_an_origin_and_commits_yields_both_aliases() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo, "one");
        commit(&repo, "two");
        git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:VoidNullable/lific.git",
            ],
        );

        let aliases = aliases_of(&repo);
        assert_eq!(kinds(&aliases), vec![AliasKind::Remote, AliasKind::Root]);
        assert_eq!(
            value_of(&aliases, &AliasKind::Remote).as_deref(),
            Some("v1:github.com/VoidNullable/lific")
        );

        let root = git_ok(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
        assert_eq!(
            value_of(&aliases, &AliasKind::Root),
            Some(format!("v1:{root}"))
        );
    }

    #[test]
    fn a_repo_without_an_origin_yields_only_the_root_alias() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo, "one");

        let aliases = aliases_of(&repo);
        assert_eq!(kinds(&aliases), vec![AliasKind::Root]);
    }

    #[test]
    fn a_repo_with_an_origin_but_no_commits_yields_only_the_remote_alias() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        git_ok(&repo, &["remote", "add", "origin", "https://host/a/b.git"]);

        let aliases = aliases_of(&repo);
        assert_eq!(kinds(&aliases), vec![AliasKind::Remote]);
        assert_eq!(
            value_of(&aliases, &AliasKind::Remote).as_deref(),
            Some("v1:host/a/b")
        );
    }

    #[test]
    fn a_shallow_clone_skips_the_root_alias_but_keeps_its_remote() {
        let tmp = tempdir();
        let source = sub(&tmp, "source");
        init_repo(&source);
        commit(&source, "one");
        commit(&source, "two");

        let clone = sub(&tmp, "clone");
        let output = Command::new("git")
            .arg("clone")
            .arg("-q")
            .arg("--depth")
            .arg("1")
            .arg(format!("file://{}", source.display()))
            .arg(&clone)
            .output()
            .expect("git clone should run");
        assert!(
            output.status.success(),
            "clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // The clone's origin is a file:// URL, which is deliberately not a
        // bindable identity. Point it at a real forge so this test exercises
        // shallowness rather than re-testing file:// rejection.
        git_ok(
            &clone,
            &["remote", "set-url", "origin", "https://host/a/b.git"],
        );
        assert_eq!(
            git_ok(&clone, &["rev-parse", "--is-shallow-repository"]),
            "true"
        );

        let aliases = aliases_of(&clone);
        assert_eq!(kinds(&aliases), vec![AliasKind::Remote]);
        assert_eq!(
            value_of(&aliases, &AliasKind::Remote).as_deref(),
            Some("v1:host/a/b")
        );
    }

    #[test]
    fn an_orphan_branch_does_not_change_the_root_alias() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo, "one");
        commit(&repo, "two");

        let before = aliases_of(&repo);

        orphan_commit(&repo, "gh-pages", "orphan");
        git_ok(&repo, &["checkout", "-q", "main"]);

        assert_eq!(aliases_of(&repo), before);
    }

    #[test]
    fn a_linked_worktree_yields_the_same_aliases_as_the_main_checkout() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo, "one");
        commit(&repo, "two");
        git_ok(&repo, &["remote", "add", "origin", "https://host/a/b.git"]);

        let worktree = sub(&tmp, "worktree");
        let worktree_arg = worktree.display().to_string();
        git_ok(&repo, &["worktree", "add", "-q", worktree_arg.as_str()]);

        assert_eq!(aliases_of(&worktree), aliases_of(&repo));
    }

    #[test]
    fn a_missing_git_binary_is_reported_as_git_not_found() {
        let tmp = tempdir();
        let repo = sub(&tmp, "repo");
        init_repo(&repo);
        commit(&repo, "one");

        let err = compute_with_git("definitely-not-git-9x", &repo)
            .expect_err("a missing git binary should be an error");
        assert!(matches!(err, RepoIdentityError::GitNotFound), "got {err:?}");
    }

    #[test]
    fn a_directory_outside_any_repository_is_reported_as_not_a_repository() {
        let tmp = tempdir();
        let dir = sub(&tmp, "plain");
        std::fs::create_dir_all(&dir).expect("create dir");
        std::fs::write(dir.join("marker"), "not a repo").expect("write marker");

        match compute(&dir) {
            Err(RepoIdentityError::NotARepository) => {}
            other => panic!("expected NotARepository, got {other:?}"),
        }
    }
}
