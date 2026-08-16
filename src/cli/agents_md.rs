//! LIF-251: the AGENTS.md block writer.
//!
//! Teaches a host repo about Lific by writing a short, marker-delimited block
//! into `./AGENTS.md`. The markers make re-runs idempotent: the block is
//! replaced in place, never duplicated, and all surrounding content is left
//! untouched. If `AGENTS.md` doesn't exist it's created with just the block.
//!
//! This is the same channel Beads' `bd init` and backlog.md's
//! `agents --update-instructions` use — AGENTS.md is Linux-Foundation-governed
//! and understood by 20+ agents, so it's how every repo that uses Lific tells
//! its agents that Lific is the tracker.

use std::path::Path;

pub const BEGIN_MARKER: &str = "<!-- lific:begin -->";
pub const END_MARKER: &str = "<!-- lific:end -->";

/// What updating AGENTS.md did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsAction {
    /// The file didn't exist and was created containing just the block.
    Created,
    /// The file existed and the Lific block was inserted (no prior block).
    Inserted,
    /// The file existed with a Lific block that was replaced in place.
    Replaced,
}

impl AgentsAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentsAction::Created => "created",
            AgentsAction::Inserted => "inserted",
            AgentsAction::Replaced => "replaced",
        }
    }
}

/// Build the marker-delimited block body. When `project` is given, the
/// identifier is baked into the CLI examples; otherwise a generic placeholder is
/// used with a note on how to discover it.
pub fn render_block(project: Option<&str>) -> String {
    let ident = project.unwrap_or("APP");
    let ident_note = if project.is_some() {
        String::new()
    } else {
        "\n> Replace `APP` with this repo's project identifier — run `lific project list` to find it.\n"
            .to_string()
    };

    format!(
        "{BEGIN_MARKER}\n\
## Issue tracking: Lific\n\
\n\
This project uses **Lific** for issue tracking and project management (local-first, \
single-binary, SQLite-backed).\n\
{ident_note}\
\n\
**Preferred access — MCP.** If a Lific MCP server is configured in your client \
(see the tools/MCP config in this repo or your global config), use it directly: \
list/create/update issues, pages, and plans through the Lific tools.\n\
\n\
**CLI fallback** (works without MCP; add `--json` for machine-readable output):\n\
\n\
```bash\n\
lific issue list --project {ident} --json      # browse issues\n\
lific issue get {ident}-1                       # one issue with relations\n\
lific issue update {ident}-1 --status done      # close an issue\n\
lific search \"auth flow\" --project {ident}      # full-text search\n\
```\n\
\n\
**Conventions:**\n\
- Mark an issue `done` as soon as you finish its work — don't leave it open.\n\
- Group related work with **modules**; fit each issue into the right module.\n\
- For multi-session work, use a **plan** so the next session can resume.\n\
- Issues are self-contained work items; keep scope tight.\n\
{END_MARKER}"
    )
}

/// Write/update the Lific block in the file at `path`. Idempotent: re-running
/// replaces the block in place. Creates the file if missing.
pub fn write(path: &Path, project: Option<&str>) -> Result<AgentsAction, String> {
    let block = render_block(project);

    let existing = match std::fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };

    let (contents, action) = match existing {
        None => (format!("{block}\n"), AgentsAction::Created),
        Some(current) => merge(&current, &block)?,
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(action)
}

/// Merge `block` into `current`. If a marker-delimited Lific block exists,
/// replace exactly that span; otherwise append the block after a blank line.
fn merge(current: &str, block: &str) -> Result<(String, AgentsAction), String> {
    if let (Some(begin), Some(end_start)) = (current.find(BEGIN_MARKER), current.find(END_MARKER)) {
        if end_start < begin {
            return Err(
                "AGENTS.md has a lific:end marker before lific:begin — refusing to edit; fix the markers by hand"
                    .into(),
            );
        }
        let end = end_start + END_MARKER.len();
        let mut out = String::with_capacity(current.len() + block.len());
        out.push_str(&current[..begin]);
        out.push_str(block);
        out.push_str(&current[end..]);
        Ok((out, AgentsAction::Replaced))
    } else {
        // Append after existing content, separated by a blank line, preserving
        // whatever was already there.
        let mut out = current.trim_end().to_string();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(block);
        out.push('\n');
        Ok((out, AgentsAction::Inserted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch root for one test. Callers work inside a `proj` subdirectory
    /// that does not exist yet, matching the original fixture, where `write`
    /// has to create the parent itself. Dropping the guard removes
    /// everything, unwinding included.
    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn block_with_project_bakes_identifier() {
        let block = render_block(Some("LIF"));
        assert!(block.contains("lific issue list --project LIF --json"));
        assert!(block.contains("lific issue update LIF-1 --status done"));
        // No placeholder note when the identifier is known.
        assert!(!block.contains("project list` to find it"));
    }

    #[test]
    fn block_without_project_uses_placeholder_and_note() {
        let block = render_block(None);
        assert!(block.contains("APP"));
        assert!(block.contains("lific project list"));
    }

    #[test]
    fn creates_file_when_absent() {
        let guard = tmp();
        let dir = guard.path().join("proj");
        let path = dir.join("AGENTS.md");
        let action = write(&path, Some("LIF")).unwrap();
        assert_eq!(action, AgentsAction::Created);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(BEGIN_MARKER));
        assert!(content.contains(END_MARKER));
        assert!(content.contains("LIF-1"));
    }

    #[test]
    fn inserts_after_existing_content_preserving_it() {
        let guard = tmp();
        let dir = guard.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");
        let original = "# My Project\n\nSome existing agent instructions.\n";
        std::fs::write(&path, original).unwrap();

        let action = write(&path, Some("LIF")).unwrap();
        assert_eq!(action, AgentsAction::Inserted);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# My Project"));
        assert!(content.contains("Some existing agent instructions."));
        assert!(content.contains(BEGIN_MARKER));
    }

    #[test]
    fn rerun_is_idempotent_single_block() {
        let guard = tmp();
        let dir = guard.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");
        std::fs::write(&path, "# Header\n\nBody.\n").unwrap();

        write(&path, Some("LIF")).unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();
        let action = write(&path, Some("LIF")).unwrap();
        assert_eq!(action, AgentsAction::Replaced);
        let after_second = std::fs::read_to_string(&path).unwrap();

        // Exactly one block, and stable output across runs.
        assert_eq!(after_first.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(after_second.matches(BEGIN_MARKER).count(), 1);
        assert_eq!(after_first, after_second);
        // Surrounding content preserved.
        assert!(after_second.starts_with("# Header"));
        assert!(after_second.contains("Body."));
    }

    #[test]
    fn replace_updates_project_identifier_and_preserves_surroundings() {
        let guard = tmp();
        let dir = guard.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");
        std::fs::write(&path, "Top.\n").unwrap();
        write(&path, Some("OLD")).unwrap();
        // Append trailing content after the block to prove it's preserved.
        let with_trailer = format!("{}\n\nTrailing note.\n", std::fs::read_to_string(&path).unwrap().trim_end());
        std::fs::write(&path, with_trailer).unwrap();

        let action = write(&path, Some("NEW")).unwrap();
        assert_eq!(action, AgentsAction::Replaced);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("--project NEW"));
        assert!(!content.contains("--project OLD"));
        assert!(content.starts_with("Top."));
        assert!(content.contains("Trailing note."));
        assert_eq!(content.matches(BEGIN_MARKER).count(), 1);
    }

    #[test]
    fn inverted_markers_error() {
        let guard = tmp();
        let dir = guard.path().join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("AGENTS.md");
        std::fs::write(&path, format!("{END_MARKER}\nx\n{BEGIN_MARKER}\n")).unwrap();
        let err = write(&path, Some("LIF")).unwrap_err();
        assert!(err.contains("marker"));
    }
}
