# Contributing to Lific

Lific accepts contributions. The project is primarily a one-person effort built heavily with AI agents, under human direction and review. AI-assisted PRs aren't just welcome, they're the dominant authorship model here. Don't hide it, and don't apologize for it. The same bar applies either way: you should be able to explain your change to a reviewer, and it has to pass the test suite.

If you want to discuss an idea before writing code, open an issue. If you have a small, focused fix ready, just send the PR. Either path is fine.

## Repository

- **Source**: [github.com/VoidNullable/lific](https://github.com/VoidNullable/lific)
- **License**: Apache-2.0
- **Stack**: Rust 2024 edition (MSRV 1.88), Svelte 5 frontend (Tailwind v4, Vite, Bun)
- **Docs**: [lific.dev/docs](https://lific.dev/docs)

## Building

The repository provides the development toolchain and project commands through
[devenv](https://devenv.sh/). From a fresh checkout, approve it:

```bash
devenv allow
```

Enter the shell explicitly for interactive work. From a regular shell, use the
same task commands directly through devenv:

```bash
devenv shell
# inside the devenv shell:
devenv test
```

Entering the default shell runs the web workspace's frozen Bun install task,
not the test suite. The same task is a prerequisite of builds and checks.
Formatting is explicit with `treefmt`; shell entry never reformats sources.
The native Bun installer is disabled because it does not support frozen
installs; there is only one installer per workspace, without a separate cache
that can outlive `node_modules`. The other JavaScript workspaces have profiles:

```bash
devenv --profile docs tasks run lific:docs:check
devenv --profile e2e tasks run lific:e2e
devenv --profile promo tasks run lific:promo:check
```

Project build tasks always build the frontend before compiling the Rust binary,
and release binaries must embed a current `web/dist/` through `rust-embed`:

```bash
devenv tasks run lific:debug-build
devenv --profile release-linux tasks run lific:release:x86_64-unknown-linux-gnu # locked dist binary
```

Start the backend and frontend together through devenv's native process manager:

```bash
devenv up
```

The two processes are intentional: Vite provides frontend hot reload and API
proxying during development, while the Rust process is the real server and
embeds `web/dist` into release binaries. The backend restarts automatically
when Rust source or Cargo configuration changes. Both bind to localhost by
default; set `VITE_HOST` and `VITE_ALLOWED_HOSTS` explicitly when remote UI
access is needed.

## Tests

```bash
devenv test
```

The Devenv test graph runs native treefmt and Clippy, all-target Rust tests,
Svelte checks and unit tests, the frontend build, and the release smoke
regression test. Documentation is checked in its profile:

```bash
devenv --profile docs tasks run lific:docs:check
```

The browser suites use their profile's frozen Bun install and build the web
prerequisite through the task graph:

```bash
devenv --profile e2e tasks run lific:e2e
```

The promo profile has the same task-graph entry point:

```bash
devenv --profile promo tasks run lific:promo:check
```

The checks exercise these behaviors:

- Rust tests cover MCP tool behavior, REST boundaries, CLI parsing and help,
  first boot and initialization on a real temporary filesystem, imports,
  exports, issue references, rate limiting, retention, previews, caller
  resolution, and error handling. Most use in-memory SQLite; `lific init`
  tests use self-cleaning on-disk temporary directories because that command
  creates files and opens a database by path.
- MCP pre-init contract tests cover server and tool discovery rejection, ping
  and ignored traffic, continued handshakes, broken-pipe termination, and EOF
  termination.
- Web unit tests cover frontend helpers and state transitions that do not need
  a browser. The web check task also typechecks the Svelte app and Vite config.
- `smoke` starts a real binary, seeds a project, and visits the core overview,
  issue, page, board, settings, and navigation routes while checking rendered
  content and browser errors.
- `archives` runs two real server instances and exercises archive export/import,
  stale downloads, permissions, oversized uploads, mobile export, and failure
  handling across admin and regular-user sessions.
- `public` checks public project, issue, and page rendering, pagination,
  sanitization of hostile Markdown/HTML, attachment URL handling, anonymous
  access, and that public pages never call credentialed private APIs.
- `sidebar`, `mobile-nav`, and `context-menu` use focused browser fixtures to
  cover responsive layout, keyboard/focus behavior, touch targets, project and
  group recovery, ordering rollback, theme readability, route reveal, and
  native modified-link behavior without requiring a full seeded application.
  The native-link cases capture CDP events and browser state so Ctrl-click and
  middle-click popup regressions retain useful failure evidence.
- The release smoke check runs each native artifact's `--version` and `--help`,
  starts it with a temporary config/database, and fetches the embedded HTML.
- `devenv test` also starts the actual backend and Vite processes through the
  native process manager, waits for their readiness probes, checks the UI and
  API proxy, then stops both. Its freshly allocated database is under
  `DEVENV_RUNTIME`, separate from the persistent development database.
- Environment regression checks verify shell/test task boundaries, formatter
  ordering, compiler consistency, and the packaged source allowlists.

The packaged frontend uses the same `web/bun.lock` as local builds and platform
releases. After intentionally updating that lockfile, regenerate its Nix
dependency manifest through the pinned Devenv tool:

```bash
devenv tasks run lific:web:lock-update
```

The test graph rejects a stale generated manifest. Nix package inputs are
explicit source files, never local databases, dependency installs, or build
caches. The package builds and embeds a fresh UI with the version from
`Cargo.toml`; it never uses or removes the checkout's `web/dist`.

Every new MCP tool and REST endpoint should ship with tests. Conventions:

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

CI checks formatting and lints with warnings-as-errors. Reproduce the complete
project check locally with:

```bash
devenv test
```

If clippy complains, fix it. Don't `#[allow]` a lint without a comment explaining why.

The devenv shell installs the repository's generated pre-commit hooks. Treefmt
and the native Clippy hook use the pinned toolchain; no separate installation
is needed. They run automatically as part of `devenv test`:

```bash
devenv test
```

Use the native treefmt integration to apply formatting fixes across the
configured languages. Run `devenv tasks run lific:check` when you need the
project check task without the test lifecycle.

## Release builds

Pushing a version tag runs the release workflow. It builds the embedded web UI
and produces locked `dist` artifacts for Linux x86_64 and aarch64,
macOS x86_64 and aarch64, and Windows x86_64 (MSVC). Linux targets use the
devenv-provided Zig linker locally; the other targets build on their native
GitHub Actions runners. The workflow verifies artifact existence, smoke-tests
native artifacts, publishes SHA-256 checksums, and attaches all five binaries
to the GitHub release.

For a local Linux cross-build:

```bash
devenv --profile release-linux tasks run lific:release:aarch64-unknown-linux-gnu
```

On macOS, `devenv` also provides both Apple Rust targets and the Apple SDK
used by the credential and SQLite stacks. Build the current Mac
architecture normally;
on Apple Silicon, build the Intel artifact with:

```bash
devenv --profile release-darwin tasks run lific:release:aarch64-apple-darwin
devenv --profile release-darwin tasks run lific:release:x86_64-apple-darwin
```

Platform signing, notarization, installers, and update channels are later
phases of the release plan, not part of this MVP.

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
