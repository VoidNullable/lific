# Publishing Lific to the official MCP Registry

Lific is listed on the official [MCP Registry](https://registry.modelcontextprotocol.io)
(`registry.modelcontextprotocol.io`, API v0.1, currently in **preview**). The
registry hosts **metadata only** — the actual artifact lives on crates.io. This
doc is the exact runbook for (re)publishing.

Tracked by LIF-253.

## What's already in the repo

- **`server.json`** (repo root) — the server manifest the registry ingests.
  Schema version `2025-12-11`.
- **README ownership marker** — a visible line in `README.md`:
  `mcp-name: io.github.VoidNullable/lific`. This is how the registry proves we
  own the `lific` crate (see "Ownership verification" below). **Do not delete
  it and do not move it into an HTML comment** — see the crates.io gotcha below.

## Key facts (verified against the registry docs, July 2026)

- **`registryType: "cargo"` is officially supported.** Rust crates on crates.io
  are a first-class package type. crates.io is the only allowed
  `registryBaseUrl` for cargo.
  Source: <https://modelcontextprotocol.io/registry/package-types> (Cargo section)
  and the base-URL allowlist in
  <https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/official-registry-requirements.md>.
- **No `runtimeHint` for cargo.** `cargo install lific` puts the binary on PATH
  at `~/.cargo/bin`; clients invoke it by name. There is no `npx`/`uvx`/`dnx`
  equivalent for cargo, so the package entry omits `runtimeHint` by design.
- **Run command is `lific mcp`.** Expressed in `server.json` as a single
  positional `packageArguments` entry (`{"type":"positional","value":"mcp"}`),
  so a client resolves the launch as `lific mcp` over stdio.

## Namespace and casing — READ THIS

The server name is **`io.github.VoidNullable/lific`** — matching the GitHub login
`VoidNullable` **with its exact capitalization**.

This is deliberate and it contradicts the naive "namespaces are lowercase"
assumption. As of July 2026 the registry's GitHub-namespace permission checks
are **case-sensitive** and the normalization fix is still open
([registry#689](https://github.com/modelcontextprotocol/registry/issues/689)).
The GitHub OIDC/device token authorizes `io.github.<login>/*` with the login's
literal case, so a lowercase `io.github.voidnullable/lific` would fail namespace
authorization until #689 lands. A real-world instance of this bug and its fix:
<https://github.com/GavinLucas/docker-mcp/pull/74>.

Three strings MUST stay identical, byte-for-byte, including case:

1. `name` in `server.json`
2. the `mcp-name:` marker in `README.md`
3. whatever namespace your `mcp-publisher login github` token grants

If #689 is resolved to force-lowercase before we publish, switch all three to
`io.github.voidnullable/lific` together. Until then, keep the capital `V` and `N`.

## Ownership verification (cargo-specific gotcha)

The registry verifies we own the `lific` crate by looking for an
`mcp-name: <server-name>` string in the crate README **as rendered on
crates.io**.

**crates.io strips HTML comments during markdown→HTML rendering.** The
`<!-- mcp-name: ... -->` hidden-comment trick that works for PyPI and NuGet
**does NOT work for cargo** — the token never appears in the HTML the validator
inspects. The marker therefore lives in `README.md` as **visible markdown text**
(a bullet under the "MCP Registry" heading). Keep it visible.
Source: <https://modelcontextprotocol.io/registry/package-types> (Cargo →
Ownership Verification).

Because crates.io serves the README that ships inside the published crate, the
marker must be present in the crate version you publish. `Cargo.toml` already
`include`s `README.md`, so `cargo publish` carries it. Publish the crate version
BEFORE (or together with) publishing to the MCP Registry, so the marker is live
when the registry fetches it.

## How publishing works now — automated on release (LIF-253)

**As of LIF-253, publishing to the MCP Registry is automated in
`.github/workflows/release.yml`.** You do not normally run `mcp-publisher` by
hand — cutting a release does it for you. The manual runbook below is the
**fallback** for when the automated job fails (a flaky registry) or you need to
re-publish out of band.

The release workflow has a `publish-registry` job that:

1. `needs: [verify, publish]` — it runs **after** the crates.io `publish` job,
   so the crate (and its `mcp-name:` ownership marker) is live before the
   registry fetches it.
2. Installs a **pinned, checksum-verified** `mcp-publisher` (currently `v1.7.9`,
   linux/amd64 SHA256 hardcoded in the workflow `env`) rather than a fetched
   `latest` binary — reproducible and supply-chain-safe. Re-pin the version and
   SHA256 together when bumping.
3. Authenticates with **GitHub OIDC** — `./mcp-publisher login github-oidc` —
   which needs `permissions: id-token: write` (set on the job). No stored
   secret or PAT: the OIDC token proves the `io.github.VoidNullable/*`
   namespace. The namespace casing must match `server.json`'s `name` and the
   README marker exactly (the check is case-sensitive — see casing section).
4. Waits (up to ~2 min) for the just-published crate version to appear on
   crates.io, since the ownership check reads the crate's README.
5. `./mcp-publisher publish`, with a retry loop that absorbs crates.io
   propagation lag but **fails fast on a 422 validation error** (a bad
   `server.json` — retrying wouldn't help).

The job is marked **`continue-on-error: true`**: a flaky or slow registry never
fails an otherwise-successful release (the binaries and crates.io publish have
already shipped by the time this job runs). If it goes yellow/failed, re-run it
manually with the runbook below.

Version sync is enforced up front: the `verify` job asserts that **both**
`server.json` version fields (top-level `version` and `packages[0].version`)
equal the release tag, so a stale `server.json` fails the whole release fast
instead of publishing the wrong version to the registry.

## Runbook (manual fallback)

### One-time: install the publisher CLI

```bash
# macOS/Linux prebuilt binary
curl -L "https://github.com/modelcontextprotocol/registry/releases/latest/download/mcp-publisher_$(uname -s | tr '[:upper:]' '[:lower:]')_$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').tar.gz" \
  | tar xz mcp-publisher && sudo mv mcp-publisher /usr/local/bin/

# or, if you have Homebrew
brew install mcp-publisher

mcp-publisher --help   # sanity check
```

### Every publish

Preconditions: the crate version you're announcing is already live on crates.io
(the release pipeline handles this — see below), and its README contains the
`mcp-name` marker.

```bash
# from the repo root, where server.json lives
mcp-publisher login github      # device-code OAuth; grants io.github.VoidNullable/*
mcp-publisher publish           # validates server.json + verifies crate ownership
```

`mcp-publisher publish` will:

1. Validate `server.json` against the schema.
2. Publish the metadata to `registry.modelcontextprotocol.io`.
3. Server-side: verify we own the `lific` crate (the README marker) and that the
   token's namespace matches `name`.

Verify it landed:

```bash
curl "https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.VoidNullable/lific"
```

### Bumping the version on each release

The registry is versioned. On every Lific release the registry's `version` must
track the shipped crate. **The `publish-registry` CI job re-publishes for you**
— you just have to keep `server.json`'s version in sync, and the `verify` job
will fail the release if you forget.

1. Bump `version` in **`Cargo.toml`** (the release source of truth).
2. Update **both** `version` fields in **`server.json`** to the same value:
   the top-level `version` and the `packages[0].version`. They must always
   equal `Cargo.toml`'s version and the release tag — the `verify` job asserts
   this and fails fast on drift.
3. Cut the release using `.github/workflows/release.yml`: update the version,
   commit it, and push the release tag. The `magi` mirror mentioned in the
   workflow is maintainer-only infrastructure and may not exist in a public
   checkout; do not add or invent that remote. The release workflow builds
   binaries, runs `cargo publish` to crates.io, and then (in
   `publish-registry`) publishes `server.json` to the MCP Registry
   automatically.
4. **Only if `publish-registry` failed** (it's `continue-on-error`, so check the
   run): re-run the manual `mcp-publisher publish` from the repo root, per
   "Every publish" above.

> Tip: keep `Cargo.toml`, `server.json` (×2), and the release tag in lockstep.
> A mismatch between the registry `version` and the crates.io version is the
> most common drift — the `verify` job now catches it before any build runs.

## CI wiring (done — LIF-253)

The automated `publish-registry` job in `.github/workflows/release.yml` is the
implementation of what this section used to describe as "later." It runs after
the crates.io `publish` job, installs a pinned+checksummed `mcp-publisher`,
authenticates via `mcp-publisher login github-oidc` (`id-token: write`), waits
for crates.io propagation, and publishes — non-fatally (`continue-on-error`).
See "How publishing works now" at the top of this doc for the details.

If the registry's namespace normalization bug ([registry#689]) ever lands and
forces lowercase, or the `server.json` schema version changes, update the three
casing-linked strings (see casing section) and re-pin the schema together, then
let the next release re-publish.

[registry#689]: https://github.com/modelcontextprotocol/registry/issues/689

## References

- Package types (incl. Cargo): <https://modelcontextprotocol.io/registry/package-types>
- Quickstart / publishing flow: <https://modelcontextprotocol.io/registry/quickstart>
- Authentication & namespaces: <https://modelcontextprotocol.io/registry/authentication>
- Official registry requirements: <https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/server-json/official-registry-requirements.md>
- server.json schema (2025-12-11): <https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json>
- Namespace case-sensitivity bug: <https://github.com/modelcontextprotocol/registry/issues/689>
