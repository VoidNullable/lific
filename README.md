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
  <strong>Issue tracking for the agentic coding era.</strong><br>
  One binary. One SQLite database (plus an attachments dir). MCP built in.
</p>

---

Your agent can write the code. What it can't do is remember: the plan dies with the context window, the TODO list rots in a markdown file, and the next session starts from zero. Lific is the missing memory: a self-hosted, single-binary issue tracker whose primary user is often an agent rather than a person.

Three numbers instead of adjectives:

- **26 MCP tools in 5,231 tokens.** That's the measured size of the full `tools/list` response at v2.0.0 (o200k tokenizer). Your entire tracker costs about as much context as one long file read. Bloated MCP servers are a real tax; this one isn't.
- **One ~25 MB binary.** Embedded SQLite, embedded web UI, backups built in. The data set is just the database and a content-addressed `attachments/` dir beside it (both covered by the automatic backups). No Docker, no Postgres, no reverse proxy, no daemon farm. Copy it to a server, point your agents at it, done.
- **11 AI clients configured by one command.** `lific connect` writes correct MCP config into OpenCode, Claude Code, Cursor, VS Code, Codex, Zed, and more. No hand-edited JSON.

Identifiers are human-readable everywhere: `APP-42`, never a UUID. They survive being spoken, logged, grepped, and pasted into a prompt.

## 60-second setup

```bash
cargo install lific     # or grab a binary from the releases page

lific init              # writes lific.toml with sensible defaults
lific start             # creates the database, serves on :3456,
                        # prints your API key once
lific connect           # writes MCP config into your AI clients
```

That's the whole thing. `lific connect` detects the AI tools installed on your machine, lets you pick, mints a per-tool API key, and merges correct MCP config into each one without overwriting existing config. Restart your client and the Lific tools are there.

The web UI is at `http://localhost:3456`. Sign up there to create your account, then grant it admin rights from the CLI: `lific user promote <username>`.

Verify any setup with:

```bash
lific doctor            # green/yellow/red checks: config, database, server,
                        # OAuth discovery, and a real MCP round-trip
```

`doctor` exits nonzero if anything is actually broken, so agents and CI can gate on it.

## What your agent can now do

- **Ask "what can I work on right now?" in one call.** `list_issues(project="APP", workable=true)` returns only issues with every blocker resolved. Dependency-aware triage without a graph query.
- **Keep a plan alive across sessions.** Plans are persistent, nestable step trees. A fresh session calls `get_plan` and resumes exactly where the last one left off. No `MEMORY.md`, no re-priming ritual.
- **Break work down and wire it up.** Create issues, link blockers (`blocks`, `relates_to`, `duplicate`), group them into modules, and mirror plan steps to real issues with two-way done/close sync.
- **Leave a real audit trail.** `get_activity` answers "what changed while I was gone": who changed what, when, and through which tool. Every agent's work is attributed (more below).
- **Write docs where the issues live.** Markdown pages in folders, with comments, labels, lifecycle status, and Mermaid diagrams. Design decisions stay next to the work they justify.
- **Edit without resending.** `edit_issue` / `edit_page` do targeted find-and-replace, so updating one line of a long description doesn't cost the whole document in tokens.
- **Take everything with you.** `export_issue`, `export_page`, `export_project`: portable markdown, no lock-in.

## Every tool gets its own identity, and that's the point

`lific connect` mints a **separate bot identity per tool**, owned by your account (`opencode-blake`, `cursor-blake`, ...). When several agents work the same project, provenance is the primitive that keeps you sane:

- **The audit log shows which harness made every change.** "OpenCode closed APP-42", "Cursor edited the design page". All attributed to you, never blurred together.
- **Revoke one tool without touching the others.** Cursor misbehaving? Disconnect just its key and OpenCode keeps working.
- **Scoped authority.** Each bot inherits its owner's project access and nothing more.

This is the recommended way to connect agent harnesses.

## Connecting AI tools

**`lific connect`** is the front door. It supports eleven clients out of the box:

`opencode` · `claude-code` · `claude-desktop` · `cursor` · `vscode` · `codex` · `zed` · `gemini` · `windsurf` · `goose` · `crush`

```bash
lific connect                                    # interactive picker
lific connect --client opencode --client cursor --yes   # non-interactive
lific connect --client claude-code --scope project      # repo-local .mcp.json
lific connect --client zed --stdio               # no server: direct SQLite over stdio
lific connect --dry-run --client vscode          # preview without writing
```

Each client gets its native schema (`mcpServers` vs `servers` vs `mcp`, Codex TOML with an env-var token, Goose YAML; the quirks are handled). JSON configs are merged non-destructively; a file `connect` can't parse safely is left untouched and you get the exact snippet to paste instead.

<details>
<summary>OAuth, if you'd rather auth as yourself</summary>

Lific implements the full MCP authorization spec (RFC 9728 protected-resource metadata, dynamic client registration, PKCE), so OAuth-capable clients can connect with **just the URL** and complete auth in the browser:

```bash
lific connect --oauth --client opencode   # writes a header-less config, mints nothing
opencode mcp auth lific                   # browser opens → sign in → approve

# or with any client's native command:
claude mcp add --transport http lific http://localhost:3456/mcp
```

The trade-off: an OAuth token **is you**. Changes made through it are indistinguishable from your own edits in the audit log, with no per-harness attribution. Fine for personally browsing your tracker from an editor; for agents doing real work, prefer the per-tool bot identities above.

**Headless / SSH / agents.** No browser on the box? The device flow has you covered:

```bash
lific login                     # prints a URL + short code; approve on any device
lific login --non-interactive   # agent mode: prints JSON {verification_uri, user_code,
                                #   device_code, next_step} and exits immediately
lific login --complete <code>   # finish later, from a script or a second session
```

Tokens are stored in your OS keyring (Secret Service / Keychain / Credential Manager), falling back to a `0600` file with a loud warning when no keyring exists.

</details>

<details>
<summary>Manual configuration (any MCP client)</summary>

**Remote (Streamable HTTP):**

```json
{
  "lific": {
    "type": "remote",
    "url": "http://localhost:3456/mcp",
    "headers": { "Authorization": "Bearer your-api-key" }
  }
}
```

**Local (stdio, no server):**

```json
{
  "lific": {
    "type": "local",
    "command": ["lific", "--db", "path/to/lific.db", "mcp"]
  }
}
```

Create keys anytime with `lific key create --name my-key`.

</details>

<details>
<summary>Web UI setup (if you prefer clicking)</summary>

Go to **Settings > Connected tools** in the web UI. Pick your tool, click Connect, and paste the generated config snippet.

Each connection creates a bot identity tied to your account (the CLI's `connect` does the same). Changes show up attributed to you, tagged with which tool made them.

</details>

## Plans: state that outlives the session

An agent's plan shouldn't die when its context does. A **plan** is an ordered, arbitrarily-nestable tree of steps that persists across sessions and compaction. Start a new session, call `get_plan`, and it's still there, ready to resume.

- **Steps can mirror issues.** Link a step to an issue and the two stay in sync: close the issue and the step checks itself; mark the step done and the issue closes. Reopen the issue and the step reopens, with a note of why.
- **Authored in one call.** `create_plan` builds a full nested tree at once; `edit_plan_step` and `update_plan_step` keep it current.
- **First-class in the UI.** A Plans tab sits alongside Issues, Board, Modules, and Pages: a real tree view with done toggles, per-step markdown notes, issue chips, and an activity timeline.
- **Fully tracked.** Every plan and step change lands in the audit log, including the issue-driven cascades.

Issues stay flat and lateral; the hierarchy lives on the plan. It's the difference between an issue tracker and a project planner.

## Built for agents, not just reachable by them

- **`lific agents-md`** writes an idempotent, marker-delimited block into your repo's `AGENTS.md` telling every agent that this project uses Lific: project identifier, CLI examples, and the workflow conventions. `lific connect` offers to do this automatically in project context.
- **Session instructions.** The MCP server ships its conventions in the `initialize` response, so connected agents know how Lific wants to be used without you explaining it.
- **Self-onboarding.** On a fresh database, the MCP tools tell the agent exactly how to bootstrap (`create a project first: manage_resource(...)`) instead of returning an empty list.
- **Pipe-native CLI.** Output auto-upgrades to JSON when piped, so `lific issue list --project APP | jq` just works, no `--json` needed (though it's there). Prompts never hang a non-interactive caller; they fail fast and name the bypass flag.
- **Shell completions:** `lific completion fish | source` (bash, zsh, fish, powershell, elvish).

The CLI also works directly against the database, no server or auth required:

```bash
lific project list
lific issue list --project APP
lific issue create --project APP --title "Fix login bug" --priority high
lific issue update APP-42 --status done
lific search "authentication" --project APP
```

## MCP tools

All 26, in 5,231 tokens:

| Family | Tools |
|--------|-------|
| Issues | `list_issues` · `get_issue` · `create_issue` · `update_issue` · `edit_issue` · `get_board` |
| Relations | `link_issues` · `unlink_issues` |
| Pages | `get_page` · `create_page` · `update_page` · `edit_page` |
| Plans | `create_plan` · `get_plan` · `edit_plan_step` · `update_plan_step` |
| Comments | `add_comment` · `list_comments` |
| Search & history | `search` · `get_activity` |
| Structure | `list_resources` · `manage_resource` · `delete` |
| Export | `export_issue` · `export_page` · `export_project` |

Everything takes human-readable identifiers (`project="APP"`, not `project_id=7`). The behaviors worth knowing about are covered in "What your agent can now do" above; for exact schemas, connect a client and read `tools/list`.

## Features

| Category | What you get |
|----------|-------------|
| **Issue tracking** | Status, priority, modules with icons, labels, relations, comments, board view, fuzzy search, sort by recent activity |
| **Plans** | Persisted, nestable step trees that outlive a session; steps mirror issues with two-way done/close sync |
| **Documentation** | Markdown pages in recursive folders, with comments, labels, lifecycle status, full-text search, and Mermaid diagrams |
| **MCP interface** | 26 tools, human-readable identifiers, compact schema, session instructions |
| **Onboarding** | `lific connect` (11 clients), `lific doctor`, `lific agents-md`, shell completions |
| **REST API** | Full CRUD for all resources, search, board view |
| **Web UI** | Markdown editing with live preview, drag-and-drop board, Mermaid and code-copy, dark/light theme |
| **User accounts** | Individual auth, per-tool bot identities, project membership and roles |
| **Auth** | OAuth 2.1 (PKCE, dynamic client registration, RFC 9728 discovery), RFC 8628 device flow, API keys, token revocation |
| **Backups** | Automatic SQLite snapshots with configurable retention |
| **CLI** | Full CRUD, TTY-aware JSON output, works with no server running |
| **Single binary** | No runtime dependencies, embedded SQLite, ~25 MB |

## When Lific is the wrong tool

Honesty is cheaper than churn:

- **You need enterprise team features.** No SSO/SAML, no sprints, no estimates, no roadmap gantt charts. If you're coordinating forty humans, use Linear or Plane.
- **You want issues as files in the repo.** Lific is a database with an API, not markdown-in-git. If you want `git diff` on your task list, a markdown-native tracker fits better.
- **You need distributed multi-writer sync.** One Lific instance is one SQLite file: a single source of truth that's trivially backed up, not a CRDT. Multiple agents talk to one server; the server doesn't merge with other servers.

For one human directing several agents across personal projects (the thing it's built for), none of those trade-offs bite.

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

CLI flags (`--db`, `--port`, `--host`) override config values. Set `server.public_url` when exposing Lific beyond localhost; it becomes the OAuth issuer and the URL `lific connect` writes into client configs.

</details>

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

## MCP Registry

Lific ships a registry manifest (`server.json`) for the official [MCP Registry](https://registry.modelcontextprotocol.io). Its canonical registry name:

- `mcp-name: io.github.VoidNullable/lific`

## License

[Apache-2.0](LICENSE)
