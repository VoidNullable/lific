<p align="center">
  <img src="LificHero.png" alt="Lific — Issue tracking built for AI-driven development" width="800">
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

Lific's full MCP schema fits in ~2,500 tokens. It uses human-readable identifiers (`APP-42`, not UUIDs), runs as a single binary with an embedded SQLite database, and includes a web UI for when you want to look at things yourself.

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

On first run, Lific generates an API key and prints it to the console. Save it — it won't be shown again. This key is used for MCP and API access.

Open `http://localhost:3456` to use the web UI. The first account you create is the admin.

## Connecting AI tools

Point your MCP client at the server. Replace `your-api-key` with the key from first run (or create one with `lific key create --name my-key`).

**Remote (network):**

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

**Local (stdio, no network):**

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

| Tool | What it does |
|------|-------------|
| `list_issues` | Filter by status, priority, module, label, or workable |
| `get_issue` | Full issue details with relations, labels, and comments |
| `create_issue` / `update_issue` | Create or partially update by identifier |
| `get_board` | Board view grouped by status, priority, or module |
| `search` | Full-text search across issues and pages |
| `link_issues` / `unlink_issues` | Dependency tracking (blocks, relates_to, duplicate) |
| `get_page` / `create_page` / `update_page` | Markdown documents in folders |
| `add_comment` / `list_comments` | Comments on issues |
| `list_resources` | Discover projects, modules, labels, folders |
| `manage_resource` | Create/update projects, modules, labels, folders |
| `delete` | Delete anything by identifier |

Everything uses human-readable identifiers: `project="APP"` not `project_id=7`.

**Workable filter:** `list_issues(project="APP", workable=true)` returns only issues with all blockers resolved — one call to answer "what can I work on right now?"

## Features

| Category | What you get |
|----------|-------------|
| **Issue tracking** | Status, priority, modules, labels, relations, comments, board view |
| **Documentation** | Markdown pages in recursive folders |
| **MCP interface** | 16 tools, ~2,500 token schema, human-readable identifiers |
| **REST API** | Full CRUD for all resources, search, board view |
| **Web UI** | Inline editing, drag-and-drop, dark/light theme |
| **User accounts** | Individual auth, per-tool bot identities, project lead permissions |
| **OAuth 2.1** | PKCE, dynamic client registration, token revocation |
| **Backups** | Automatic SQLite snapshots with configurable retention |
| **Single binary** | No runtime dependencies, embedded SQLite, ~15MB |

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

## Building from source

### Requirements

- **Rust 1.88+** (edition 2024) required
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
