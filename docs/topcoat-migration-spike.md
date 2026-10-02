# Topcoat 0.9 migration spike

## Decision

**Experiment result:** the spike mounts one Topcoat page as Axum's unmatched-request fallback behind the `topcoat-spike` Cargo feature. The default build keeps serving the existing embedded Svelte app. The feature build preserves the existing Axum route table and proves that `/api/health` remains reachable.

Topcoat 0.9.0 is pinned exactly. Its crates declare Rust 1.98 MSRV, while Lific's devenv and `rust-version` are 1.88. Enabling the spike feature therefore requires Rust 1.98 or newer; the ordinary build remains on 1.88. Before production adoption, the project must decide to raise the supported toolchain and update devenv/CI/package builders together.

## Run the experiment

With Rust 1.98.1 installed alongside the repository's devenv toolchain, run the named smoke tests from the repository root:

```sh
devenv shell -- sh -c 'RUSTC="$(rustup which rustc --toolchain 1.98.1)" RUSTDOC="$(rustup which rustdoc --toolchain 1.98.1)" cargo test --features topcoat-spike topcoat_spike_ -- --nocapture'
```

The Topcoat fallback route is `/__topcoat-spike`; the existing Axum health route is `/api/health`.

## Hosting topology

Keep Axum as the one listener and owner of REST, MCP, OAuth, WebSocket, public routes, middleware, CORS, and compression. Axum sends unmatched requests to Topcoat's `TowerService`. This is a small reversible seam and lets routes move independently while API ownership stays in place.

Topcoat's default asset bundling writes generated assets beside the executable. Lific currently embeds `web/dist` into its single binary. A production cutover must either configure and verify a single-binary-compatible Topcoat asset strategy or change every package/install/upgrade path to ship the generated asset directory. Do not assume the current embedded frontend packaging will include Topcoat assets.

## Authentication boundary

This spike does not read credentials, call private APIs, alter storage, or introduce sessions. The page has no API behavior. Topcoat procedures do not provide the per-call custom-header behavior needed to transparently reuse Lific's current `localStorage['lific_token']` bearer contract, so do not move the token into server-global or Topcoat session state.

LIF-203 should define a browser API adapter contract before feature ports begin:

- Private REST calls read `localStorage['lific_token']` at call time and attach it as `Authorization: Bearer …`; token absence sends no bearer. Preserve request-scoped backend identity resolution.
- Public REST calls use `credentials: 'omit'` and send no bearer header, so cookies and private identity cannot leak into anonymous routes.
- WebSockets remain connected to the Axum endpoint and retain its existing same-origin session-cookie and origin validation.
- Prove separation with two distinct browser users and two distinct bearer tokens, then verify public requests carry neither token nor cookie. Add browser coverage before switching a user-facing feature to Topcoat.

The repo has Playwright end-to-end tests, but this static route spike does not add a browser fetch shim or fake one with server-side state. That behavior belongs in LIF-203 after the shared client and browser contract are designed.

## Scope and known gaps

- The feature builds one server-rendered page and proves the Axum health route survives the fallback mount.
- It does not port existing UI, prove runtime interaction parity, move asset packaging, or validate API/WebSocket auth from a browser.
- Topcoat 0.9 is documented as early-stage and experimental, and its client runtime is described as highly experimental. Pin it and reassess on each planned upgrade.
- Keep `topcoat-spike` disabled by default. The production go/no-go depends on the Rust MSRV decision, single-binary asset strategy, auth adapter contract, and the route-by-route parity evidence tracked in the migration plan.

## Branch-added tests

- `topcoat_spike_route_renders_its_server_page` sends a request through Topcoat's Tower adapter and checks that the discovered Topcoat page renders its identifying heading.
- `topcoat_spike_fallback_preserves_axum_health_route` builds the assembled Lific server, requests `/api/health` and the Topcoat page, and checks that Axum still serves health while Topcoat serves the unmatched frontend path.

## References

- [Topcoat 0.9.0 release](https://github.com/tokio-rs/topcoat/releases/tag/v0.9.0)
- [Topcoat Tower adapter](https://docs.rs/topcoat/0.9.0/topcoat/router/tower/index.html)
- [Topcoat asset guide](https://github.com/tokio-rs/topcoat/blob/v0.9.0/crates/topcoat/docs/asset.md)
- [Topcoat project status](https://github.com/tokio-rs/topcoat)
