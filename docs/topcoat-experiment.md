# Topcoat experiment scaffold

The opt-in `topcoat-spike` Cargo feature mounts a small Topcoat 0.9.0 app as
the existing Axum router's fallback. The default feature set and embedded
Svelte frontend remain the normal application path. The sample page is only
available when the experiment feature is enabled:

The project pins Rust 1.99.0 in Cargo metadata and devenv; CI uses the same
toolchain. Topcoat 0.9.0 itself requires Rust 1.98 or newer.

- `GET /__topcoat-spike` renders the sample page inside the shared document
  layout.
- `GET /__topcoat-spike.css` serves the scaffold stylesheet with a CSS content
  type.
- Existing Axum routes such as `/api/health` continue to be handled before the
  Topcoat fallback.

## Local commands

Use the Topcoat profile to install the CLI matching the pinned Topcoat library
version and format the Topcoat macro bodies:

```sh
devenv --profile topcoat tasks run lific:topcoat:install-cli
devenv --profile topcoat tasks run lific:topcoat:fmt
```

The install task uses `cargo install --locked --version 0.9.0 topcoat-cli` and
stores the executable under the devenv Cargo install root. The formatter task
uses that executable directly, so it does not depend on an unrelated global
Topcoat CLI.

Build and run the opt-in app through the Topcoat profile, which disables Cargo's
default Vite feature:

```sh
devenv --profile topcoat shell -- cargo run --locked --no-default-features --features topcoat-spike -- start
```

The scaffold stylesheet is currently a static Topcoat route backed by
`include_str!`, so it is embedded in the executable and does not need an asset
directory. Its route and CSS response are exercised by the feature-gated test
suite with `devenv --profile topcoat tasks run lific:topcoat:test`.

## API adapter

`server::topcoat_api` provides the feature-gated Topcoat frontend with a typed
HTTP client for the existing `/api` routes. Its project and issue DTOs mirror
the JSON contract and are separate from the database models. Required nullable
fields stay required on the wire, and unknown fields fail decoding. Issue DTOs
also retain the server's `seq`, import `source`, and optional `waits` fields.

The client builds authenticated requests, encodes query parameters, serializes
JSON bodies, and decodes JSON responses. Multipart requests use reqwest's form
builder to create the content-type boundary. Downloads return the response
bytes and content headers. `send_json_with_headers` returns decoded data with
the HTTP status and headers; `send_json` remains the data-only convenience
method. This preserves pagination metadata such as `x-comment-has-more`. API
errors retain the HTTP status and server message. If the response body fails
after headers arrive, the typed body-read error keeps that status. Stale-write
conflicts also retain the `current` entity and `update_conflict` code so a
caller can reconcile and retry with `expected_seq`.

The focused tests cover project and issue JSON compatibility, missing required
fields, conflict response recovery data, response-body failures across JSON
and download paths, pagination header retention, bearer headers, query
encoding, JSON mutation bodies with `expected_seq`, multipart boundary
generation, and authenticated download request construction.

The existing Svelte transport still has several behaviors that migrated
screens must account for: shared JSON requests use `requestWithHeaders`, normal
downloads use `download`, archive downloads use a separate abortable `fetch`,
and archive imports plus attachment uploads use `XMLHttpRequest` to report
upload progress. The adapter does not replace those Svelte call sites yet.

Topcoat's `asset!` API uses a separate generated bundle. When a migrated screen
starts using it, `topcoat asset bundle --bin lific` writes to
`target/debug/assets` by default, beside the executable that the CLI built. A
release executable needs a bundle produced from the same release build; ship
that executable with its adjacent `assets/` directory. Do not copy a bundle
between debug/release builds or different build directories.
