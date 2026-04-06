<p><img src="IssyLogo.png" width="80" alt="Lific"></p>

# Lific

A lightweight issue tracker designed for AI-driven development. 

If you use AI assistants to manage your projects (Claude, OpenCode, Cursor, etc.), Lific gives them a fast, structured issue tracker with a small set of focused tools for managing issues, pages, and project workflows.

## Why

Most issue trackers were built for teams of people clicking around a web UI. They work fine for that. But if your primary interface is an AI assistant managing issues through MCP, the priorities are different:

- **Schema size matters.** Every tool definition eats context window tokens. Lific exposes 14 tools in ~2,000 tokens. Plane's MCP server for example exposes 100+ tools at 80,000+ tokens.
- **Identifiers should be readable.** `APP-42` instead of `5a61c25e-96ae-43f0-b25c-570ecefcb772`.
- **Setup should be nothing.** Single binary, SQLite database, no external dependencies.
- **Your data is yours.** SQLite file on your disk. Copy it, back it up, inspect it with any SQL tool.

## Install

```
cargo install lific
```

Or grab a binary from [GitHub Releases](https://github.com/Void-n-Null/lific/releases).

## Quickstart

```bash
lific init          # creates lific.toml with defaults
lific start         # starts the server on port 3456
```

Open the web UI at `http://localhost:3456` and create an account. The first account is automatically an admin.

## Connecting AI tools

Once you have an account, go to **Settings > Connected Tools** in the web UI. Pick your tool (OpenCode, Cursor, Claude Code, Claude Desktop, Codex) and click Connect. Lific generates a bot identity that acts on your behalf and gives you the config snippet to paste into your tool's MCP configuration.

Each connection creates a bot account tied to your user. Comments and changes made by the bot show up as coming from that tool, attributed to you.

### Manual / headless setup

If you're running Lific on a remote server without the web UI, you can also connect via the REST API or point your MCP client directly at the `/mcp` endpoint with a bearer token.

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

### Local stdio (no network)

```json
{
  "lific": {
    "type": "local",
    "command": ["lific", "--db", "path/to/lific.db", "mcp"]
  }
}
```

## Web UI

Lific includes a web interface served from the same binary. No separate frontend deployment needed.

- **Issues** with filters, search, inline editing, markdown descriptions, comments
- **Pages** as markdown documents with a recursive folder tree and drag-and-drop organization
- **Project management** with create, edit, delete, lead assignment, and icon picker
- Dark/light theme with system preference detection

The UI auto-links issue and page identifiers in markdown. Writing `LIF-42` in a description turns it into a clickable link.

## What the AI sees

Lific gives your AI assistant the following tools:


| Tool                                       | What it does                                                                                    |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `list_resources`                           | Discover projects, modules, labels, folders, pages, issues                                      |
| `list_issues`                              | Filter by status, priority, module, label, or workable                                          |
| `get_issue`                                | Full details for a specific issue with relations and labels                                     |
| `create_issue`                             | Create with project, module, labels, priority                                                   |
| `update_issue`                             | Partial updates by identifier                                                                   |
| `get_board`                                | Board view grouped by status, priority, or module                                               |
| `search`                                   | Full-text search across issues and pages                                                        |
| `link_issues` / `unlink_issues`            | Dependency tracking (blocks, relates_to, duplicate)                                             |
| `get_page` / `create_page` / `update_page` | Markdown docs                                                                                   |
| `manage_resource`                          | Create/update projects, modules, labels, folders                                                |
| `delete`                                   | Delete anything by identifier                                                                   |


Everything uses human-readable identifiers. `project="APP"`, not `project_id=7`. `module="Backend"`, not `module_id=3`.

### Workable filter

`list_issues(project="APP", workable=true)` returns only issues where all blockers are resolved. One call to answer "what can I actually start right now?" instead of manually tracing dependency chains.

## What's in, what's coming

**Shipping now:**
- Projects, issues, labels, modules
- Issue relations (blocks, relates_to, duplicate)
- Markdown pages organized in recursive folders
- Comments on issues
- Web UI with inline editing, drag-and-drop, dark/light theme
- Full-text search across everything
- Board view (status/priority/module grouping)
- User accounts with bot identities per connected tool
- OAuth 2.1 for external clients
- Automatic SQLite backups

**Planned:**
- Milestones with changelog generation
- Git-aware issue references (parse commit messages for identifiers)
- Activity log per issue
- File attachments on issues and pages
- Webhooks on issue changes
- VS Code extension
- Real-time updates via WebSocket

**Not planned:**
Sprints, story points, custom fields, workflow automations. If you need those, this isn't for you.

## Backups

Lific automatically backs up the database using SQLite's online backup API. Interval and retention are configurable in `lific.toml`. On shutdown, the WAL is checkpointed so the `.db` file is always self-contained and safe to copy.

## Configuration

`lific init` creates a `lific.toml`:

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

## Building from source

```bash
git clone https://github.com/Void-n-Null/lific
cd lific
cargo build --release
```

The frontend is built separately and embedded in the binary:

```bash
cd web && bun install && bun run build && cd ..
cargo build --release
```

Requires Rust 2024 edition. SQLite is bundled (no system dependency).

## License

MIT
