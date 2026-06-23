<p align="center">
  <img src="LificHero.png" alt="Lific Issue tracking built for AI-driven development" width="800">
</p>

<p align="center">
  <a href="https://github.com/VoidNullable/lific/actions/workflows/ci.yml"><img src="https://github.com/VoidNullable/lific/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/lific"><img src="https://img.shields.io/crates/v/lific" alt="crates.io"></a>
  <a href="https://github.com/VoidNullable/lific/releases"><img src="https://img.shields.io/github/v/release/VoidNullable/lific" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/VoidNullable/lific" alt="License"></a>
</p>

<p align="center">
  <strong>Lightweight issue tracking built for AI-driven development.</strong><br>
  Single binary. MCP built in. Clean web UI.
</p>

---

Lific's full MCP schema fits in roughly 5,200 tokens. It uses human-readable identifiers (`APP-42`, not UUIDs), runs as a single binary with an embedded SQLite database, and includes a web UI for when you want to look at things yourself.

## Install

```bash
cargo install lific
```

Or grab a binary from the [releases page](https://github.com/VoidNullable/lific/releases).

## Quickstart

```bash
lific init     # creates lific.toml + lific.db
lific start    # starts on port 3456
```

On first run, Lific generates an API key and prints it to the console. It won't be shown again. This key is used for MCP and API access.

Open `http://localhost:3456` to use the web UI. The first account you create is the admin.

The CLI also works directly against the database. No server or auth required:

```bash
lific project list
lific issue list --project APP
lific issue create --project APP --title "Fix login bug" --priority high
lific issue get APP-42
lific issue update APP-42 --status done
lific search "authentication" --project APP
```

Add `--json` to any command for machine-readable output.

## Connecting AI tools

Point your MCP client at the server. Replace `your-api-key` with the key from first run (or create one with `lific key create --name my-key`).

**Access MCP commands:**

```json
{
  "lific": {
    "type": "remote",
    "url": "http://localhost:3456/mcp",
    "headers": {
      "Authorization": "Bearer your-api-key"
    }
  }
}
```

**Access CLI commands:**

```json
{
  "lific": {
    "type": "local",
    "command": ["lific", "--db", "path/to/lific.db", "mcp"]
  }
}
```

<details>
<summary>Web UI setup (if you prefer clicking)</summary>

Go to **Settings > Connected Tools** in the web UI. Pick your tool, click Connect, and paste the generated config snippet. Supported tools: OpenCode, Cursor, Claude Code, Claude Desktop, Codex.

Each connection creates a bot identity tied to your account. Changes show up attributed to you, tagged with which tool made them.

</details>

## MCP tools

| Tool                                              | What it does                                                                         |
| ------------------------------------------------- | ------------------------------------------------------------------------------------ |
| `list_issues`                                     | Filter by status, priority, module, label, or workable                               |
| `get_issue`                                       | Full issue details with relations, labels, and comments                              |
| `create_issue` / `update_issue`                   | Create or partially update by identifier                                             |
| `edit_issue` / `edit_page`                        | Targeted find-and-replace edits without resending the whole body                     |
| `get_board`                                       | Board view grouped by status, priority, or module                                    |
| `search`                                          | Fuzzy full-text search across issues and pages                                       |
| `get_activity`                                    | Audit history for an issue, page, or whole project — who changed what, when          |
| `link_issues` / `unlink_issues`                   | Dependency tracking (blocks, relates_to, duplicate)                                  |
| `get_page` / `create_page` / `update_page`        | Markdown documents in folders, with labels and lifecycle status                      |
| `add_comment` / `list_comments`                   | Threaded comments on issues and pages                                                |
| `create_plan` / `get_plan`                        | Persisted, nestable step plans that survive across sessions; steps can mirror issues |
| `edit_plan_step` / `update_plan_step`             | Edit a step, toggle done (closing a mirrored issue), add/move/delete steps           |
| `list_resources`                                  | Discover projects, modules, labels, folders, plans                                   |
| `manage_resource`                                 | Create/update projects, modules (with icons), labels, folders                        |
| `delete`                                          | Delete anything by identifier                                                        |
| `export_issue` / `export_page` / `export_project` | Export to portable markdown                                                          |

Everything uses human-readable identifiers: `project="APP"` not `project_id=7`.

**Workable filter:** `list_issues(project="APP", workable=true)` returns only issues with all blockers resolved. One call can answer "what can I work on right now?"

## Plans

An agent's plan shouldn't die when its context does. A **plan** is an ordered, arbitrarily-nestable tree of steps that **persists across sessions and compaction** — start a new session and the plan is still there, ready to resume.

- **Steps can mirror issues.** Link a step to an issue and the two stay in sync: close the issue and the step checks itself; mark the step done and the issue closes. Reopen the issue and the step reopens, with a note of why.
- **Authored in one call.** `create_plan` builds a full nested tree at once; `get_plan` rehydrates it for the next session; `edit_plan_step` and `update_plan_step` keep it current.
- **First-class in the UI.** A Plans tab sits alongside Issues, Board, Modules, and Pages — a real tree view with done toggles, per-step markdown notes, issue chips, and an activity timeline.
- **Fully tracked.** Every plan and step change lands in the audit log, including the issue-driven cascades.

Issues stay flat and lateral; the hierarchy lives on the plan. It's the difference between an issue tracker and a project planner.

## Features

| Category           | What you get                                                                                                                                        |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Issue tracking** | Status, priority, modules with icons, labels, relations, comments, board view, fuzzy search, sort by recent activity                                |
| **Plans**          | Persisted, nestable step trees that outlive a session; steps mirror issues with two-way done/close sync; first-class tree view and activity history |
| **Documentation**  | Markdown pages in recursive folders, with comments, labels, lifecycle status, full-text search, and Mermaid diagrams                                |
| **MCP interface**  | 26 tools, human-readable identifiers, compact schema                                                                                                |
| **REST API**       | Full CRUD for all resources, search, board view                                                                                                     |
| **Web UI**         | Markdown editing with live preview, drag-and-drop board, Mermaid and code-copy, dark/light theme                                                    |
| **User accounts**  | Individual auth, per-tool bot identities, project lead permissions                                                                                  |
| **OAuth 2.1**      | PKCE, dynamic client registration, token revocation, per-user token identity                                                                        |
| **Backups**        | Automatic SQLite snapshots with configurable retention                                                                                              |
| **CLI**            | Full CRUD for issues, projects, pages, modules, labels, folders. No server needed                                                                   |
| **Single binary**  | No runtime dependencies, embedded SQLite, ~16MB                                                                                                     |

## Configuration

<details>
<summary><code>lific.toml</code></summary>

`lific init` generates this:

```toml
[server]
host = "0.0.0.0"
port = 3456

[database]
path = "lific.db"

[backup]
enabled = true
dir = "backups"
interval_minutes = 60
retain = 24

[log]
level = "info"
```

CLI flags (`--db`, `--port`, `--host`) override config values.

</details>

### Deployment-aware issuer derivation

Lific automatically derives the OAuth issuer URL from incoming request headers, so it works correctly whether you're connecting locally or over the network:

| Deployment       | How you connect                | Issuer derivation                                                     |
| ---------------- | ------------------------------ | --------------------------------------------------------------------- |
| Local dev        | `http://127.0.0.1:3456`        | From `Host` header → `http://127.0.0.1:3456`                          |
| Tailscale funnel | `https://yourname.ts.net:3456` | From `X-Forwarded-Proto` + `Host` → `https://yourname.ts.net:3456`    |
| Reverse proxy    | `https://example.com/lific`    | From `X-Forwarded-Proto` + `X-Forwarded-Host` → `https://example.com` |
| Direct IP        | `http://203.0.113.7:3456`      | From `Host` header → `http://203.0.113.7:3456`                        |

This ensures OAuth works without manual configuration — the protected-resource metadata always matches what your AI tool connects to.

For advanced setups (e.g., behind a proxy that strips headers), set `public_url` explicitly:

```toml
[server]
host = "127.0.0.1"
port = 3456
public_url = "https://example.com/lific"
```

The `public_url` takes precedence over header-based derivation.

## Building from source

### Requirements

- **Rust 1.88+** required
- **Bun** optional, only needed if you want the web UI

SQLite is bundled via `rusqlite` and compiled into the binary. No system SQLite required.

### API-only build (no web UI)

```bash
git clone https://github.com/VoidNullable/lific
cd lific
mkdir -p web/dist
cargo build --release
```

The `mkdir -p web/dist` creates the empty directory that `rust-embed` expects at compile time. The resulting binary has full functionality (MCP, REST API, CLI, OAuth, backups) but visiting the web UI will return a message pointing you to build the frontend.

### Full build (with web UI)

```bash
git clone https://github.com/VoidNullable/lific
cd lific
cd web && bun install && bun run build && cd ..
cargo build --release
```

The frontend is a Svelte 5 SPA built with Vite. `bun run build` outputs static files to `web/dist/`, which `cargo build` embeds into the binary. The final binary is fully self-contained with no runtime dependencies.

## Contributing

Issues and PRs welcome. If you're planning something big, open an issue first so we can talk about it before you put in the work.

## License

[Apache-2.0](LICENSE)
