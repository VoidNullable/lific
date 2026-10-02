# Topcoat 0.9 migration spike

## Decision

The `topcoat-spike` Cargo feature mounts one Topcoat page as Axum's unmatched-request fallback. The default build serves the existing embedded Svelte app. The feature build keeps the existing Axum routes, including `/api/health`.

Topcoat 0.9.0 is pinned exactly. Its crates declare Rust 1.98 MSRV; Lific's devenv and `rust-version` are 1.88. The spike feature requires Rust 1.98 or newer, while the ordinary build remains on 1.88. Production adoption requires raising the supported toolchain and updating devenv, CI, and package builders together.

## Run the experiment

With Rust 1.98.1 installed alongside the repository's devenv toolchain, run the named smoke tests from the repository root:

```sh
devenv shell -- sh -c 'RUSTC="$(rustup which rustc --toolchain 1.98.1)" RUSTDOC="$(rustup which rustdoc --toolchain 1.98.1)" cargo test --features topcoat-spike topcoat_spike_ -- --nocapture'
```

The Topcoat fallback route is `/__topcoat-spike`; the existing Axum health route is `/api/health`.

## Hosting topology

Axum remains the listener for REST, MCP, OAuth, WebSocket, and public routes, and continues to own middleware, CORS, and compression. It sends unmatched requests to Topcoat's `TowerService`. This seam lets frontend routes move independently while APIs remain in Axum.

Topcoat's default asset bundling writes generated assets beside the executable. Lific embeds `web/dist` in its single binary. A production cutover must either configure and verify a single-binary-compatible Topcoat asset strategy or update every package, install, and upgrade path to ship the generated asset directory. The current embedded frontend packaging does not include Topcoat assets.

## Authentication boundary

This spike does not read credentials, call private APIs, alter storage, or introduce sessions. The page has no API behavior. Topcoat procedures do not support the per-call custom headers needed to reuse Lific's `localStorage['lific_token']` bearer contract. The token must not move into server-global or Topcoat session state.

The browser API adapter work defines this contract before feature ports begin:

- Private REST calls read `localStorage['lific_token']` at call time and attach it as `Authorization: Bearer …`; token absence sends no bearer. Preserve request-scoped backend identity resolution.
- Public REST calls use `credentials: 'omit'` and send no bearer header, so cookies and private identity cannot leak into anonymous routes.
- WebSockets remain connected to the Axum endpoint and retain its existing same-origin session-cookie and origin validation.
- Prove separation with two distinct browser users and two distinct bearer tokens, then verify public requests carry neither token nor cookie. Add browser coverage before switching a user-facing feature to Topcoat.

The repo has Playwright end-to-end tests. This static route spike adds no browser fetch shim or server-side substitute. The browser API adapter work owns that behavior after defining the shared client and browser contract.

## Scope and known gaps

- The feature builds one server-rendered page and confirms the Axum health route remains available through the fallback mount.
- It does not port existing UI, prove runtime interaction parity, move asset packaging, or validate API/WebSocket auth from a browser.
- Topcoat 0.9 and its client runtime are documented as experimental. Keep the version pinned and reassess each planned upgrade.
- Keep `topcoat-spike` disabled by default. The production go/no-go depends on the Rust MSRV decision, single-binary asset strategy, auth adapter contract, and the route-by-route parity evidence tracked in the migration plan.

## Branch-added tests

- `topcoat_spike_route_renders_its_server_page` sends a request through Topcoat's Tower adapter and checks that the discovered Topcoat page renders its identifying heading.
- `topcoat_spike_fallback_preserves_axum_health_route` builds the assembled Lific server, requests `/api/health` and the Topcoat page, and checks that Axum still serves health while Topcoat serves the unmatched frontend path.

## References

- [Topcoat 0.9.0 release](https://github.com/tokio-rs/topcoat/releases/tag/v0.9.0)
- [Topcoat Tower adapter](https://docs.rs/topcoat/0.9.0/topcoat/router/tower/index.html)
- [Topcoat asset guide](https://github.com/tokio-rs/topcoat/blob/v0.9.0/crates/topcoat/docs/asset.md)
- [Topcoat project status](https://github.com/tokio-rs/topcoat)
