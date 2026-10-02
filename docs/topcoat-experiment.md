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

Enter the repository's devenv shell, then install the CLI matching the pinned
Topcoat library version and format the Topcoat macro bodies:

```sh
devenv shell
devenv tasks run lific:topcoat:install-cli
devenv tasks run lific:topcoat:fmt
```

The install task uses `cargo install --locked --version 0.9.0 topcoat-cli` and
stores the executable under the devenv Cargo install root. The formatter task
uses that executable directly, so it does not depend on an unrelated global
Topcoat CLI.

Build and run the opt-in app with:

```sh
cargo run --locked --features topcoat-spike -- start
```

The scaffold stylesheet is currently a static Topcoat route backed by
`include_str!`, so it is embedded in the executable and does not need an asset
directory. Its route and CSS response are exercised by the feature-gated test
suite in devenv.

Topcoat's `asset!` API uses a separate generated bundle. When a migrated screen
starts using it, `topcoat asset bundle --bin lific` writes to
`target/debug/assets` by default, beside the executable that the CLI built. A
release executable needs a bundle produced from the same release build; ship
that executable with its adjacent `assets/` directory. Do not copy a bundle
between debug/release builds or different build directories.
