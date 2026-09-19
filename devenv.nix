{
  pkgs,
  config,
  lib,
  inputs,
  ...
}:
let
  repoRoot = if config.git.root != null then config.git.root else builtins.toString ./.;
  lificVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  bun2nix = inputs.bun2nix.packages.${pkgs.stdenv.hostPlatform.system}.default;
  rustPlatform = pkgs.makeRustPlatform {
    cargo = config.languages.rust.toolchainPackage;
    rustc = config.languages.rust.toolchainPackage;
  };
  source =
    paths:
    lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions paths;
    };
  webBundle = pkgs.stdenv.mkDerivation {
    pname = "lific-web";
    version = lificVersion;
    # The browser bundle must be reproducible from source, never copied from
    # a developer's checkout.
    # Vite reads Cargo.toml for the version displayed in the UI.
    src = source [
      ./Cargo.toml
      ./web/package.json
      ./web/bun.lock
      ./web/index.html
      ./web/vite.config.ts
      ./web/svelte.config.js
      ./web/tsconfig.json
      ./web/tsconfig.app.json
      ./web/tsconfig.node.json
      ./web/src
      ./web/public
    ];
    nativeBuildInputs = [
      bun2nix.hook
      config.languages.javascript.package
    ];
    bunRoot = "web";
    bunInstallFlags = [
      "--frozen-lockfile"
      "--linker=hoisted"
    ]
    ++ lib.optionals pkgs.stdenv.isDarwin [ "--backend=copyfile" ];
    bunDeps = bun2nix.fetchBunDeps { bunNix = ./web/bun.nix; };
    buildPhase = ''
      runHook preBuild
      (cd web && bun run build)
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p "$out"
      cp -R web/dist/. "$out/"
      runHook postInstall
    '';
  };
  lificPackage = rustPlatform.buildRustPackage {
    pname = "lific";
    version = lificVersion;
    src = source [
      ./Cargo.toml
      ./Cargo.lock
      ./build.rs
      ./src
      ./migrations
      ./LICENSE
      ./README.md
    ];
    cargoLock.lockFile = ./Cargo.lock;
    buildType = "dist";
    # This is the derivation's private source copy. The checkout is never
    # modified: every release package embeds the UI built by webBundle.
    postPatch = ''
      mkdir -p web/dist
      cp -R ${webBundle}/. web/dist/
    '';
    doCheck = false;
  };
  playwrightBrowsers = pkgs.playwright-driver.browsers.override {
    withChromium = true;
    withChromiumHeadlessShell = true;
    withFirefox = false;
    withWebkit = false;
    withFfmpeg = false;
  };
  playwrightChromium = pkgs.writeShellScript "lific-playwright-chromium" ''
    set -euo pipefail
    for executable in \
      "$PLAYWRIGHT_BROWSERS_PATH"/chromium-*/chrome-linux*/chrome \
      "$PLAYWRIGHT_BROWSERS_PATH"/chromium-*/chrome-mac*/Google\ Chrome\ for\ Testing.app/Contents/MacOS/Google\ Chrome\ for\ Testing; do
      if [ -x "$executable" ]; then
        exec "$executable" "$@"
      fi
    done
    echo "devenv Chromium executable not found" >&2
    exit 1
  '';
  chromiumRuntimePackages = with pkgs; [
    alsa-lib
    at-spi2-atk
    atk
    cairo
    cups
    dbus
    expat
    fontconfig
    freetype
    glib
    gtk3
    libdrm
    libxkbcommon
    libxshmfence
    mesa
    nspr
    nss
    pango
    wayland
    xorg.libX11
    xorg.libXcomposite
    xorg.libXdamage
    xorg.libXext
    xorg.libXfixes
    xorg.libXi
    xorg.libXrandr
    xorg.libXrender
    xorg.libXcursor
    xorg.libXtst
    xorg.libxcb
  ];
  lockedBunInstall = workspace: ''
    lock="${config.devenv.state}/locks/lific-${workspace}-bun-install"
    mkdir -p "$(dirname "$lock")"
    deadline=$((SECONDS + 300))
    while ! mkdir "$lock" 2>/dev/null; do
      owner=""
      if [[ -f "$lock/pid" ]]; then
        owner="$(<"$lock/pid")"
      fi
      if [[ -n "$owner" ]] && ! kill -0 "$owner" 2>/dev/null; then
        rm -f "$lock/pid"
        rmdir "$lock" 2>/dev/null || true
        continue
      fi
      if (( SECONDS >= deadline )); then
        echo "timed out waiting for the ${workspace} Bun install lock" >&2
        exit 1
      fi
      sleep 1
    done
    printf '%s\n' "$$" > "$lock/pid"
    cleanup() {
      rm -f "$lock/pid"
      rmdir "$lock" 2>/dev/null || true
    }
    trap cleanup EXIT INT TERM
    bun install --frozen-lockfile
  '';
in
{
  languages.rust = {
    enable = true;
    channel = "stable";
    version = "1.88.0";
  };

  languages.javascript = {
    enable = true;
    directory = "${repoRoot}/web";
    bun = {
      enable = true;
      # The native installer cannot enforce --frozen-lockfile. Use one task
      # per workspace for both shell entry and direct task invocations.
      install.enable = false;
    };
  };

  # devenv's JavaScript module supports one project directory per environment.
  # Each profile adds an explicit frozen install prerequisite so direct task
  # invocations are reproducible without relying on shell entry.
  profiles = {
    docs.module = {
      # Documentation needs Bun, not the Rust toolchain or source hooks.
      languages.rust.enable = lib.mkForce false;
      treefmt.enable = lib.mkForce false;
      git-hooks.hooks.clippy.enable = lib.mkForce false;
      git-hooks.hooks.treefmt.enable = lib.mkForce false;
      languages.javascript.directory = "${repoRoot}/site";
    };
    e2e.module = {
      languages.javascript.directory = "${repoRoot}/e2e";
      packages = [ playwrightBrowsers ];
      env.PLAYWRIGHT_BROWSERS_PATH = "${playwrightBrowsers}";
      env.PLAYWRIGHT_EXECUTABLE_PATH = "${playwrightChromium}";
      tasks = {
        "lific:install:e2e" = {
          cwd = "${repoRoot}/e2e";
          exec = lockedBunInstall "e2e";
          before = [ "devenv:enterShell" ];
        };
        # Backend suites share the debug binary; component suites only need
        # Vite and Chromium. Keeping those groups separate lets the task graph
        # run them concurrently without changing the test scripts.
        "lific:e2e:app" = {
          cwd = "${repoRoot}/e2e";
          exec = ''
            bun run smoke
            bun run archives
            bun run public
          '';
          after = [
            "lific:debug-build"
            "lific:install:e2e"
          ];
        };
        "lific:e2e:components" = {
          cwd = "${repoRoot}/e2e";
          exec = ''
            bun run sidebar
            bun run mobile-nav
            bun run context-menu
          '';
          after = [
            "lific:web:build"
            "lific:install:e2e"
          ];
        };
        "lific:e2e" = {
          after = [ "lific:e2e:app" "lific:e2e:components" ];
        };
      };
    };
    promo.module = {
      languages.javascript.directory = "${repoRoot}/promo";
      packages = pkgs.lib.optionals pkgs.stdenv.isLinux chromiumRuntimePackages;
      env.LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.isLinux (
        pkgs.lib.makeLibraryPath chromiumRuntimePackages
      );
      tasks = {
        "lific:install:promo" = {
          cwd = "${repoRoot}/promo";
          exec = lockedBunInstall "promo";
          before = [ "devenv:enterShell" ];
        };
        "lific:promo:check" = {
          cwd = "${repoRoot}/promo";
          exec = "bun run lint";
          after = [ "lific:install:promo" ];
        };
        "lific:promo:render" = {
          cwd = "${repoRoot}/promo";
          exec = "bunx remotion render BoardLoop ${repoRoot}/site/public/board-loop.mp4";
          after = [ "lific:promo:check" ];
        };
      };
    };
    release-linux.module = {
      languages.rust.targets = [
        "x86_64-unknown-linux-gnu"
        "aarch64-unknown-linux-gnu"
      ];
      unsetEnvVars = [
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"
        "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER"
      ];
      languages.zig = {
        enable = true;
        lsp.enable = false;
      };
      packages = [ pkgs.cargo-zigbuild ];
      tasks = {
        "lific:release:x86_64-unknown-linux-gnu" = {
          cwd = repoRoot;
          exec = "cargo zigbuild --locked --profile dist --target x86_64-unknown-linux-gnu";
          after = [ "lific:web:build" ];
        };
        "lific:release:aarch64-unknown-linux-gnu" = {
          cwd = repoRoot;
          exec = "cargo zigbuild --locked --profile dist --target aarch64-unknown-linux-gnu";
          after = [ "lific:web:build" ];
        };
      };
    };
    release-darwin.module = {
      languages.rust.targets = [
        "x86_64-apple-darwin"
        "aarch64-apple-darwin"
      ];
      tasks = {
        "lific:release:x86_64-apple-darwin" = {
          cwd = repoRoot;
          exec = "cargo build --locked --profile dist --target x86_64-apple-darwin";
          after = [ "lific:web:build" ];
        };
        "lific:release:aarch64-apple-darwin" = {
          cwd = repoRoot;
          exec = "cargo build --locked --profile dist --target aarch64-apple-darwin";
          after = [ "lific:web:build" ];
        };
      };
    };
    release-windows.module = {
      languages.rust.targets = [ "x86_64-pc-windows-gnu" ];
      unsetEnvVars = [ "CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER" ];
      languages.zig = {
        enable = true;
        lsp.enable = false;
      };
      packages = [ pkgs.cargo-zigbuild ];
      tasks."lific:release:x86_64-pc-windows-gnu" = {
        cwd = repoRoot;
        exec = "cargo zigbuild --locked --profile dist --target x86_64-pc-windows-gnu";
        after = [ "lific:web:build" ];
      };
    };
  };

  treefmt = {
    enable = true;
    config.programs = {
      actionlint.enable = true;
      # Enable the source formatters in the optional formatting baseline, not
      # as part of the Devenv migration.
      # nixfmt.enable = true;
      # prettier.enable = true;
      rustfmt.enable = true;
      rustfmt.package = config.languages.rust.toolchainPackage;
      # shfmt.enable = true;
    };
    config.settings.excludes = [
      "web/dist/*"
      "site/.next/*"
      "promo/out/*"
      "target/*"
      "web/bun.nix"
    ];
  };

  outputs = {
    lific = lificPackage;
    web = webBundle;
  };

  packages = [
    bun2nix
  ]
  ++ (with pkgs; [
    curl
    file
    git
  ]);

  env.CARGO_TERM_COLOR = "always";
  env.RUST_BACKTRACE = "1";
  unsetEnvVars = [ "RUSTC_WRAPPER" ];
  tasks = {
    # Devenv traverses dependents as well as prerequisites on shell entry.
    # Only attach the test graph when actually running `devenv test`.
    "devenv:git-hooks:run" = {
      before = lib.mkForce (lib.optionals config.devenv.isTesting [ "devenv:enterTest" ]);
      after = [ "lific:web:build" ] ++ lib.optionals config.devenv.isTesting [ "lific:web:check" ];
    };
    "devenv:treefmt:run" = {
      # Formatting is explicit in development and checked before CI builds.
      # Shell entry must not silently repair a future formatting failure.
      before = lib.mkForce (lib.optionals config.devenv.isTesting [ "devenv:enterTest" ]);
      exec = lib.mkForce "treefmt --ci";
    };
    "lific:install:web" = {
      cwd = "${repoRoot}/web";
      exec = lockedBunInstall "web";
      before = lib.optionals (config.languages.javascript.directory == "${repoRoot}/web") [
        "devenv:enterShell"
      ];
    };
    "lific:install:site" = {
      cwd = "${repoRoot}/site";
      exec = lockedBunInstall "site";
      before = lib.optionals (config.languages.javascript.directory == "${repoRoot}/site") [
        "devenv:enterShell"
      ];
    };
    "lific:docs:build" = {
      cwd = "${repoRoot}/site";
      exec = "bun run build";
      after = [ "lific:install:site" ];
    };
    "lific:docs:check" = {
      cwd = repoRoot;
      exec = "bun scripts/check-docs.mjs";
      after = [ "lific:docs:build" ];
    };
    "lific:web:lock-check" = {
      cwd = "${repoRoot}/web";
      exec = ''
        generated="$(mktemp)"
        trap 'rm -f "$generated"' EXIT
        bun2nix -o "$generated"
        diff -u bun.nix "$generated"
      '';
    };
    "lific:web:lock-update" = {
      cwd = "${repoRoot}/web";
      exec = "bun2nix -o bun.nix";
    };
    "lific:rust-test" = {
      cwd = repoRoot;
      exec = "cargo test --all-targets --locked";
      after = [ "lific:web:build" ] ++ lib.optionals config.devenv.isTesting [ "lific:web:check" ];
    };
    "lific:web:check" = {
      cwd = "${repoRoot}/web";
      exec = "bun run check && bun test";
      after = [ "lific:install:web" ] ++ lib.optionals config.devenv.isTesting [ "devenv:treefmt:run" ];
    };
    "lific:community-proxy:check" = {
      cwd = repoRoot;
      exec = "bun test ./deploy/community-redirect/worker.test.mjs";
      after = [ "lific:install:web" ];
    };
    "lific:web:build" = {
      cwd = "${repoRoot}/web";
      exec = ''
        bun run build
        # Vite clears dist before writing the bundle; keep the tracked checkout
        # marker so the generated tree remains safe for native git hooks.
        touch dist/.gitkeep
      '';
      after = [ "lific:install:web" ];
    };
    "lific:release-test" = {
      cwd = repoRoot;
      exec = "bash scripts/verify-release-binary.test.sh";
    };
    "lific:devenv-test" = {
      cwd = repoRoot;
      exec = "bun test scripts/devenv.test.ts";
    };
    "lific:publish" = {
      cwd = repoRoot;
      exec = ''
        set -o pipefail
        if out="$(cargo publish --locked --allow-dirty --no-verify 2>&1)"; then
          echo "$out"
        else
          echo "$out"
          if printf '%s\n' "$out" | grep -qiE 'crate version .* is already uploaded'; then
            echo "::notice::Version already on crates.io; treating as success."
          else
            echo "::error::cargo publish failed."
            exit 1
          fi
        fi
      '';
      after = [ "lific:web:build" ];
    };
    "lific:check" = {
      before = lib.optionals config.devenv.isTesting [ "devenv:enterTest" ];
      after = [
        "lific:rust-test"
        "lific:web:check"
        "lific:release-test"
        "lific:web:lock-check"
        "lific:community-proxy:check"
        "lific:devenv-test"
      ];
    };
    "lific:debug-build" = {
      cwd = repoRoot;
      exec = "cargo build --locked";
      after = [ "lific:web:build" ];
    };
  };

  processes = {
    backend = {
      exec = ''
        ${lib.optionalString config.devenv.isTesting ''
          # Devenv owns the test process lifetime; the database lives in its
          # isolated runtime directory, never in the developer's state.
          export LIFIC_DEV_DB="$(mktemp -d "$DEVENV_RUNTIME/lific-test.XXXXXX")/lific.db"
        ''}
        exec cargo run --locked -- \
          --db "$LIFIC_DEV_DB" \
          start --init-if-missing --host 127.0.0.1 \
          --port "$LIFIC_DEV_PORT"
      '';
      cwd = repoRoot;
      after = [ "lific:debug-build" ];
      ports.http.allocate = 3456;
      env = {
        LIFIC_INIT_ADMIN_NAME = "Devenv";
        LIFIC_INIT_ADMIN_PASSWORD = "devenv-local-password";
        LIFIC_DEV_PORT = builtins.toString config.processes.backend.ports.http.value;
        LIFIC_DEV_DB = "${config.devenv.state}/lific.db";
      };
      ready.http.get = {
        port = config.processes.backend.ports.http.value;
        path = "/api/health";
      };
      ready.period = 1;
      ready.timeout = 600;
      watch = {
        paths = [
          ./src
          ./migrations
          ./Cargo.toml
          ./Cargo.lock
          ./build.rs
        ];
        extensions = [
          "rs"
          "toml"
          "lock"
          "sql"
        ];
        ignore = [ "target" ];
      };
    };
    frontend = {
      exec = "bun run dev";
      cwd = "${repoRoot}/web";
      ports.http.allocate = 5173;
      env.VITE_PORT = builtins.toString config.processes.frontend.ports.http.value;
      env.VITE_API_TARGET = "http://127.0.0.1:${builtins.toString config.processes.backend.ports.http.value}";
      after = [ "devenv:processes:backend@ready" ];
      ready.http.get = {
        port = config.processes.frontend.ports.http.value;
        path = "/";
      };
      ready.timeout = 30;
    };
  };

  enterTest = ''
    wait_for_processes 60
    curl --fail --silent --show-error --connect-timeout 1 --max-time 5 \
      http://127.0.0.1:${toString config.processes.backend.ports.http.value}/api/health
    curl --fail --silent --show-error --connect-timeout 1 --max-time 5 \
      http://127.0.0.1:${toString config.processes.frontend.ports.http.value}/ | grep -q '<html'
    curl --fail --silent --show-error --connect-timeout 1 --max-time 5 \
      http://127.0.0.1:${toString config.processes.frontend.ports.http.value}/api/health
  '';

  git-hooks.hooks = {
    treefmt.enable = true;
    clippy = {
      enable = true;
      settings = {
        denyWarnings = true;
        offline = false;
        extraArgs = "--all-targets --locked";
      };
    };
  };
}
