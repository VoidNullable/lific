# Lific

A lightweight issue tracker designed for AI-driven development. 

If you use AI assistants to manage your projects (Claude, OpenCode, Cursor, etc.), Lific gives them a fast, structured issue tracker that gives a few high quality tools for managing a project's issue workflow.

## Why

Most issue trackers were built for teams of people clicking around a web UI. They work fine for that. But if your primary interface is an AI assistant managing issues through MCP, the priorities are different:

- **Schema size matters.** Every tool definition eats context window tokens. Lific exposes 14 tools in ~2,000 tokens. Plane's MCP server for example exposes 100+ tools at 80,000+ tokens.
- **Identifiers should be readable.** `APP-42` instead of `5a61c25e-96ae-43f0-b25c-570ecefcb772`.
- **Setup should be nothing.** Simple setup and basic authentication connect you securely without overhead
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

On first start, Lific generates an API key and prints it once. Save it somewhere.

## Connecting AI Assistants

### OpenCode / Claude Code

Add to your MCP config (local stdio, no network):

```json
{
  "lific": {
    "type": "local",
    "command": ["lific", "--db", "path/to/lific.db", "mcp"]
  }
}
```

### Remote (Tailscale, VPS, etc.)

`lific start` serves both a REST API and an MCP endpoint on the same port:

- REST API: `http://localhost:3456/api/*`
- MCP: `http://localhost:3456/mcp`

Point your MCP client at the `/mcp` URL with a bearer token:

```json
{
  "lific": {
    "type": "remote",
    "url": "https://your-server/mcp",
    "headers": {
      "Authorization": "Bearer lific_sk-live-..."
    }
  }
}
```

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
| `search`                                   | Full-text search across issues and pages. Ideal for having an agent onboard onto a new project. |
| `link_issues` / `unlink_issues`            | Dependency tracking (blocks, relates_to, duplicate)                                             |
| `get_page` / `create_page` / `update_page` | Markdown docs                                                                                   |
| `manage_resource`                          | Create/update projects, modules, labels, folders                                                |
| `delete`                                   | Delete anything by identifier                                                                   |


Everything uses human-readable identifiers. `project="APP"`, not `project_id=7`. `module="Backend"`, not `module_id=3`.

### Workable filter

`list_issues(project="APP", workable=true)` returns only issues where all blockers are resolved. One call to answer "what can I actually start right now?" instead of manually tracing dependency chains.

## Data model

Projects contain issues, modules, labels, pages, and folders. Issues have status (`backlog`, `todo`, `active`, `done`, `cancelled`), priority (`urgent`, `high`, `medium`, `low`, `none`), and optional module/label assignments. Issues link to each other with `blocks`, `relates_to`, and `duplicate` relations.

Pages are markdown documents with identifiers like `APP-DOC-1`.

That's it. No sprints, no story points, no custom fields, no workflows. If you need those, this isn't for you.

## API keys

```bash
lific key create --name "my-laptop"    # prints key once, stores hash
lific key list                          # shows names and status, never keys
lific key revoke --name "my-laptop"     # instant invalidation
lific key rotate --name "my-laptop"     # revoke old, generate new
```

Keys use Argon2id hashing with BLAKE3 checksums. Only hashes are stored. Multiple keys for different clients. The health endpoint (`/api/health`) is the only unauthenticated route.

## Backups

Lific automatically backs up the database using SQLite's online backup API. Interval and retention are configurable in `lific.toml` (see Configuration below). On shutdown, the WAL is checkpointed so the `.db` file is always self-contained and safe to copy.

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

Requires Rust 2024 edition. SQLite is bundled (no system dependency).

## Importing from Plane

```bash
lific import-plane \
  --url http://localhost:8585 \
  --api-key plane_api_xxx \
  --workspace my-workspace
```

Fetches projects, modules, labels, issues, and relations from the Plane API and imports them. Use `--skip {indentifier}` to exclude specific projects by identifier.

## License

MIT