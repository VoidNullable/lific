# Topcoat 0.9 migration spike

## Decision

The `topcoat-spike` Cargo feature mounts Topcoat as Axum's unmatched-request fallback and enables its browser runtime. The default build serves the existing embedded Svelte app. The Topcoat build keeps the existing Axum routes, including `/api/health`, and excludes the Vite frontend.

Topcoat 0.9.0 is pinned exactly. Its crates declare Rust 1.98 MSRV; Lific's devenv and `rust-version` use Rust 1.99.0, which satisfies that requirement. The default and spike builds use the same pinned toolchain.

## Run the experiment

Run the named smoke tests in the Vite-free Topcoat profile with the repository's pinned Rust 1.99.0 toolchain:

```sh
devenv --profile topcoat shell -- cargo test --locked --no-default-features --features topcoat-spike topcoat_spike_ -- --nocapture
```

The Topcoat fallback route is `/__topcoat-spike`; the existing Axum health route is `/api/health`.

Format Rust with `cargo fmt`. Format Topcoat macros with the pinned CLI task:

```sh
devenv --profile topcoat tasks run lific:topcoat:fmt
```

## Hosting topology

Axum remains the listener for REST, MCP, OAuth, WebSocket, and public routes, and continues to own middleware, CORS, and compression. It sends unmatched requests to Topcoat's `TowerService`. This seam lets frontend routes move independently while APIs remain in Axum.

Lific embeds the pinned Topcoat browser runtime in the binary and serves it from the shared document layout. The `lific-topcoat` package and Topcoat build tasks do not build or embed `web/dist`. Future assets that use Topcoat's generated asset catalog still need a single-binary packaging strategy before production cutover.

## Authentication boundary

The session bridge reads the existing `localStorage['lific_token']` value at request time. It keeps the token in browser state, injects bearer credentials only for private API calls, and rewrites the supported public reads without credentials. Role data controls presentation only; the backend remains the authorization boundary. The typed Rust API adapter owns the origin and wire DTOs.

The shared request boundary preserves this contract:

- Private REST calls read `localStorage['lific_token']` at call time and attach it as `Authorization: Bearer …`; token absence sends no bearer. Preserve request-scoped backend identity resolution.
- Public REST calls use `credentials: 'omit'` and send no bearer header, so cookies and private identity cannot leak into anonymous routes.
- WebSockets remain connected to the Axum endpoint and retain its existing same-origin session-cookie and origin validation.
- Prove separation with two distinct browser users and two distinct bearer tokens, then verify public requests carry neither token nor cookie. Add browser coverage before switching a user-facing feature to Topcoat.

The embedded Topcoat runtime dispatches rendered event attributes. Session, preference, and runtime scripts are served by Topcoat routes; private/public API behavior remains in the browser session bridge rather than server-global state.

## Scope and known gaps

- The feature builds a server-rendered page, loads the embedded runtime, and confirms the Axum health route remains available through the fallback mount.
- It does not port existing product screens or replace the existing same-origin WebSocket contract.
- Topcoat 0.9 and its client runtime are documented as experimental. Keep the version pinned and reassess each planned upgrade.
- Keep `topcoat-spike` opt-in. Use `--no-default-features --features topcoat-spike`; Cargo rejects enabling the Topcoat and Vite frontends together.

## Branch-added tests

- `topcoat_spike_route_renders_its_server_page` sends a request through Topcoat's Tower adapter and checks that the discovered Topcoat page renders its identifying heading.
- `topcoat_spike_fallback_preserves_axum_health_route` builds the assembled Lific server, requests `/api/health` and the Topcoat page, and checks that Axum still serves health while Topcoat serves the unmatched frontend path.

## References

- [Topcoat 0.9.0 release](https://github.com/tokio-rs/topcoat/releases/tag/v0.9.0)
- [Topcoat Tower adapter](https://docs.rs/topcoat/0.9.0/topcoat/router/tower/index.html)
- [Topcoat asset guide](https://github.com/tokio-rs/topcoat/blob/v0.9.0/crates/topcoat/docs/asset.md)
- [Topcoat project status](https://github.com/tokio-rs/topcoat)
