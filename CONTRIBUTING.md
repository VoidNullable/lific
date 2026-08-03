# Contributing to Lific

Lific accepts contributions. The project is primarily a one-person effort built heavily with AI agents, under human direction and review. AI-assisted PRs aren't just welcome, they're the dominant authorship model here. Don't hide it, and don't apologize for it. The same bar applies either way: you should be able to explain your change to a reviewer, and it has to pass the test suite.

If you want to discuss an idea before writing code, open an issue. If you have a small, focused fix ready, just send the PR. Either path is fine.

## Repository

- **Source**: [github.com/VoidNullable/lific](https://github.com/VoidNullable/lific)
- **License**: Apache-2.0
- **Stack**: Rust 2024 edition (MSRV 1.88), Svelte 5 frontend (Tailwind v4, Vite, Bun)
- **Docs**: [lific.dev/docs](https://lific.dev/docs)

## Building

The Rust binary compiles without the frontend. If `web/dist/` doesn't exist, create it before building; the binary embeds whatever is in that directory via `rust-embed`:

```bash
mkdir -p web/dist     # only if web/dist/ doesn't exist
cargo build           # debug
cargo build --release
```

For a full build with the web UI (requires [Bun](https://bun.sh)):

```bash
cd web && bun install && bun run build && cd ..
cargo build --release
```

## Tests

```bash
cargo test
```

The whole suite runs in seconds. Every new MCP tool and REST endpoint ships with tests. Conventions:

- All tests use in-memory SQLite via `crate::db::open_memory()`.
- **Exception:** full-command tests of `lific init` (_`cmd_init`_) exercise the
  real filesystem and a genuine on-disk DB, because init writes a config file
  and opens the DB from a path — it cannot run against an in-memory database.
  These use a self-cleaning temp dir (`TempDir` in `init_target_tests`) and
  stay out of the repo tree.
- MCP tool tests call methods directly via `Parameters(...)` on a `LificMcp` instance.
- REST API tests use `tower::ServiceExt::oneshot` against the axum router.
- Test names describe behavior, not implementation.

## What CI runs (run it before pushing)

CI lints with warnings-as-errors, so `cargo test` passing is not enough. Reproduce CI locally with both commands:

```bash
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

If clippy complains, fix it. Don't `#[allow]` a lint without a comment explaining why.

## Commit message style

Conventional-commits style, with a Lific issue identifier in parens where applicable:

```
type(scope): short description (LIF-NNN)

Body explains the WHY and any non-obvious WHAT. Bullet lists are fine.
```

- **Types**: `feat`, `fix`, `perf`, `test`, `refactor`, `chore`, `docs`. Pick the closest one.
- **Scope** is freeform. Common scopes: `auth`, `mcp`, `db`, `plans`, `theme`, `issues`, `oauth`, `api`.
- The issue reference (e.g. `(LIF-215)`) goes at the end of the subject line. External contributors usually won't have one; that's fine, leave it off.

Examples from the log:

```
feat(auth): single-user web auto-login (LIF-215)
perf(plans): 2x faster list_plans via page-then-aggregate
fix(theme): clear WCAG AA for --text-faint, add --warn-text token
```

A short subject with no body is fine for trivial changes. A non-trivial change should explain the why, not just describe the diff.

## Pull requests

- Open against `master`.
- Title and description should make the user-facing change obvious. Link related issues.
- Make sure CI is green before requesting review.
- AI-generated PR descriptions are fine. So are AI-generated commit messages, code, and tests.
- **Don't update `CHANGELOG.md`**; it's generated from commit history.
- No DCO, no CLA, no signoff requirement.

## Before adding something big

Lific has a deliberately fixed scope: single binary, SQLite, MCP-native, agent-first defaults. Things that are out of scope on purpose include Docker-required deployment, sprints/estimates-style project management, and multi-writer database backends. If your idea pushes against any of that, open an issue first so we can talk it through before you put in the work.

## Reporting bugs and requesting features

File an issue on GitHub. For bugs, include reproduction steps and the version (`lific --version`). For features, describe the use case (agent workflow, human workflow, or both) and any constraints you have in mind.
