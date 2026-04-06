<p align="center">
  <img src="IssyLogo.png" alt="Lific" width="120">
</p>

<h3 align="center">Lific</h3>

<p align="center">
  Issue tracking built for AI-driven development.<br>
  Single binary. SQLite. 14 MCP tools in ~2,000 tokens.
</p>

<p align="center">
  <a href="https://github.com/Void-n-Null/lific/actions/workflows/ci.yml"><img src="https://github.com/Void-n-Null/lific/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/Void-n-Null/lific/releases"><img src="https://img.shields.io/github/v/release/Void-n-Null/lific" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Void-n-Null/lific" alt="License"></a>
</p>

---

Most issue trackers ship 100+ tools and 80,000+ tokens of schema for AI assistants to parse. Lific ships 14 tools in ~2,000 tokens. It uses human-readable identifiers (`APP-42`, not UUIDs), runs as a single binary with an embedded SQLite database, and includes a web UI for when you want to look at things yourself.

- **Issues** with status, priority, modules, labels, relations, and comments
- **Pages** as markdown documents in recursive folders
- **Web UI** with inline editing, drag-and-drop, dark/light theme
- **MCP + REST API** for AI assistants and automation
- **User accounts** with per-tool bot identities
- **Automatic backups** with configurable retention

## Install

```bash
cargo install lific
```

Or grab a binary from the [releases page](https://github.com/Void-n-Null/lific/releases).

## Quickstart

```bash
lific init     # creates lific.toml
lific start    # starts on port 3456
```

Open `http://localhost:3456`, create an account (first account is admin), and you're running.

## Connecting your AI tools

Go to **Settings > Connected Tools** in the web UI. Pick your tool, click Connect, paste the generated config snippet. Supported out of the box:

- OpenCode
- Cursor
- Claude Code
- Claude Desktop
- Codex

Each connection creates a bot identity tied to your account. Changes show up attributed to you, tagged with which tool made them.

<details>
<summary>Manual setup (headless / remote server)</summary>

Point your MCP client at the `/mcp` endpoint:

```json
{
  "lific": {
    "type": "remote",
    "url": "https://your-server/mcp",
    "headers": {
      "Authorization": "Bearer your-api-key"
    }
  }
}
```

Or run locally via stdio (no network):

```json
{
  "lific": {
    "type": "local",
    "command": ["lific", "--db", "path/to/lific.db", "mcp"]
  }
}
```

</details>

## MCP tools

<details>
<summary>14 tools, ~2,000 tokens of schema</summary>

| Tool | What it does |
|------|-------------|
| `list_resources` | Discover projects, modules, labels, folders, pages, issues |
| `list_issues` | Filter by status, priority, module, label, or workable |
| `get_issue` | Full issue details with relations and labels |
| `create_issue` | Create with project, module, labels, priority |
| `update_issue` | Partial updates by identifier |
| `get_board` | Board view grouped by status, priority, or module |
| `search` | Full-text search across issues and pages |
| `link_issues` / `unlink_issues` | Dependency tracking (blocks, relates_to, duplicate) |
| `get_page` / `create_page` / `update_page` | Markdown documents |
| `manage_resource` | Create/update projects, modules, labels, folders |
| `delete` | Delete anything by identifier |

Everything uses human-readable identifiers: `project="APP"` not `project_id=7`.

**Workable filter:** `list_issues(project="APP", workable=true)` returns only issues with all blockers resolved. One call to answer "what can I start right now?"

</details>

## Roadmap

- [x] Projects, issues, labels, modules, relations
- [x] Markdown pages in recursive folders
- [x] Comments on issues
- [x] Web UI with inline editing and drag-and-drop
- [x] Full-text search
- [x] User accounts with bot identities
- [x] OAuth 2.1
- [x] Automatic SQLite backups
- [ ] Milestones with changelog generation
- [ ] Git-aware issue references (parse commits for identifiers)
- [ ] Activity log per issue
- [ ] File attachments
- [ ] Webhooks
- [ ] VS Code extension
- [ ] Real-time updates via WebSocket

Not planned: sprints, story points, custom fields, workflow automations.

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

```bash
git clone https://github.com/Void-n-Null/lific
cd lific
cd web && bun install && bun run build && cd ..
cargo build --release
```

Requires Rust 2024 edition. SQLite is bundled.

## Contributing

Issues and PRs welcome. If you're planning something big, open an issue first so we can talk about it before you put in the work.

## License

[MIT](LICENSE)
