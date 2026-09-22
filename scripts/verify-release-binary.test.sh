#!/usr/bin/env bash
# Regression tests for verify-release-binary.sh.
#
# Each case drives the verifier against a stand-in "lific" that behaves like a
# specific broken (or correct) release build, so the verifier's own guarantees
# are checked without cutting a release. Every child process here is started by
# a case and reaped by the trap below.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
verifier="$script_dir/verify-release-binary.sh"
scratch="$(mktemp -d)"
occupied_pid=""
cleanup() {
  if [[ -n $occupied_pid ]]; then
    kill "$occupied_pid" 2>/dev/null || true
    wait "$occupied_pid" 2>/dev/null || true
  fi
  cd / || true
  rm -rf "$scratch"
}
trap cleanup EXIT

free_port() {
  local candidate
  for _ in {1..20}; do
    candidate="$(bun -e 'const server = Bun.serve({ port: 0, fetch: () => new Response() }); console.log(server.port); server.stop();' 2>/dev/null)"
    if [[ $candidate =~ ^[0-9]+$ ]]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
  echo "could not allocate a free verification port" >&2
  exit 1
}

# A stand-in web server. MODE picks which release build it imitates:
#   full        - carries the web UI, serves the bundles it names
#   no-assets   - serves a shell that names no bundles (built without web/dist)
#   spa         - names bundles but does not have them, so /assets falls back
#                 to index.html with 200 text/html
fixture_server="$scratch/fixture-server.ts"
cat >"$fixture_server" <<'TS'
const mode = process.env.MODE ?? "full";
const shell = (withAssets: boolean) =>
  `<!doctype html><html lang="en"><head><title>Lific</title>` +
  (withAssets
    ? `<script type="module" crossorigin src="/assets/index-abc123.js"></script>` +
      `<link rel="stylesheet" crossorigin href="/assets/index-abc123.css">`
    : "") +
  `</head><body><div id="app"></div></body></html>`;

const html = (withAssets: boolean) =>
  new Response(shell(withAssets), { headers: { "content-type": "text/html" } });

Bun.serve({
  port: Number(process.env.PORT),
  hostname: "127.0.0.1",
  fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/api/health") {
      return new Response("ok", { headers: { "content-type": "text/plain" } });
    }
    if (mode === "full" && path === "/assets/index-abc123.js") {
      return new Response("export const ok = 1;\n", {
        headers: { "content-type": "text/javascript" },
      });
    }
    if (mode === "full" && path === "/assets/index-abc123.css") {
      return new Response(":root{--ok:1}\n", {
        headers: { "content-type": "text/css" },
      });
    }
    // Everything else, including a missing asset, gets the SPA fallback.
    return html(mode !== "no-assets");
  },
});
console.error("lific server started");
TS

# A stand-in binary. It refuses to start the way the real one would fail if the
# verifier leaked the caller's environment into the run: without an explicit
# --config it would pick up a project-local lific.toml, and it must not be
# looking at files in the directory the verifier was invoked from.
fixture_binary="$scratch/fixture-lific"
cat >"$fixture_binary" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

config=""
port=""
previous=""
start=false
for argument in "$@"; do
  case $previous in
    --config) config=$argument ;;
    --port) port=$argument ;;
  esac
  if [[ $argument == start ]]; then
    start=true
  fi
  previous=$argument
done

case " $* " in
  *" --version "*) echo "lific test"; exit 0 ;;
  *" --help "*) exit 0 ;;
esac

if [[ $start != true ]]; then
  exit 0
fi

if [[ -e hostile-marker ]]; then
  echo "started in the caller's working directory" >&2
  exit 1
fi
if [[ -z $config ]]; then
  echo "no --config given, so a discovered lific.toml would win" >&2
  exit 1
fi
if ! grep -q '^\[database\]' "$config"; then
  echo "config $config is not the isolated one the verifier should write" >&2
  exit 1
fi

echo "lific server started" >&2
exec env PORT="${port:?}" MODE="${FIXTURE_MODE:-full}" bun "$FIXTURE_SERVER"
SH
chmod +x "$fixture_binary"

run_verifier() {
  # Usage: run_verifier <workdir> <binary-argument> [env assignments...]
  local workdir="$1" binary_argument="$2"
  shift 2
  local status=0
  (
    cd "$workdir"
    env FIXTURE_SERVER="$fixture_server" "$@" \
      "$timeout_command" 60 bash "$verifier" "$binary_argument"
  ) >"$scratch/run.log" 2>&1 || status=$?
  printf '%s\n' "$status"
}

timeout_command=timeout
if ! command -v "$timeout_command" >/dev/null 2>&1; then
  timeout_command=gtimeout
fi
if ! command -v "$timeout_command" >/dev/null 2>&1; then
  echo "could not find a timeout command for the fixtures" >&2
  exit 1
fi

fail() {
  cat "$scratch/run.log" >&2
  echo "$1" >&2
  exit 1
}

# 1. A good build passes. The binary is named relatively, from a directory that
#    is not where the verifier runs the server, so a verifier that forgot to
#    resolve the path before changing directory fails here.
mkdir -p "$scratch/launch"
cp "$fixture_binary" "$scratch/launch/lific"
status="$(run_verifier "$scratch/launch" "./lific" FIXTURE_MODE=full)"
if [[ $status -ne 0 ]]; then
  fail "verifier rejected a binary that serves its API and bundles"
fi
if ! grep -Fq "release binary serves its API and the embedded web UI" "$scratch/run.log"; then
  fail "verifier passed without reporting what it proved"
fi
echo "a complete build passes, addressed by a relative path"

# 2. A shell with no bundle references is a binary built without web/dist.
status="$(run_verifier "$scratch" "$fixture_binary" FIXTURE_MODE=no-assets)"
if [[ $status -eq 0 ]]; then
  fail "verifier accepted a web root that names no JS or CSS bundle"
fi
if [[ $status -eq 124 ]]; then
  fail "verifier hung on a placeholder web root"
fi
echo "a placeholder web root without bundles is rejected"

# 3. Named bundles that are not in the binary come back as 200 text/html.
status="$(run_verifier "$scratch" "$fixture_binary" FIXTURE_MODE=spa)"
if [[ $status -eq 0 ]]; then
  fail "verifier accepted the SPA fallback in place of a missing bundle"
fi
if [[ $status -eq 124 ]]; then
  fail "verifier hung on a missing bundle"
fi
echo "a missing bundle served as SPA HTML is rejected"

# 4. A hostile working directory (its own lific.toml, its own files) must not
#    reach the run: the verifier resolves the binary, moves to a scratch
#    directory, and passes an explicit --config.
hostile="$scratch/hostile"
mkdir -p "$hostile"
printf '%s\n' 'this is not valid toml {{{' >"$hostile/lific.toml"
: >"$hostile/hostile-marker"
status="$(run_verifier "$hostile" "$fixture_binary" FIXTURE_MODE=full)"
if [[ $status -ne 0 ]]; then
  fail "verifier leaked the caller's working directory or config into the run"
fi
echo "a hostile working directory and its lific.toml are not inherited"

# 5. A server that binds and then never answers must fail inside the budget.
no_response_server="$scratch/no-response-server.ts"
printf '%s\n' \
  'Bun.serve({ port: Number(process.env.PORT), fetch: () => new Promise(() => {}) });' \
  'console.error("lific server started");' \
  'console.error("no-response fixture listening");' \
  >"$no_response_server"

status="$(run_verifier "$scratch" "$fixture_binary" \
  FIXTURE_SERVER="$no_response_server" LIFIC_VERIFY_STARTUP_TIMEOUT=3)"
if [[ $status -eq 124 ]]; then
  fail "verifier hung on a non-responding server"
fi
if [[ $status -eq 0 ]]; then
  fail "verifier accepted a non-responding server"
fi
if ! grep -Fq "no-response fixture listening" "$scratch/run.log"; then
  fail "non-responding fixture did not bind its port"
fi
echo "a non-responding server is rejected within the startup budget"

# 6. An explicitly requested port that is already serving HTTP is refused
#    before anything is started.
port="$(free_port)"
PORT="$port" bun -e 'Bun.serve({ port: Number(process.env.PORT), fetch: () => new Response("<html>occupied</html>", { headers: { "content-type": "text/html" } }) });' &
occupied_pid=$!
for _ in {1..20}; do
  if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/" >/dev/null; then
    break
  fi
  sleep 0.1
done
curl --fail --silent --max-time 1 "http://127.0.0.1:$port/" >/dev/null

status="$(run_verifier "$scratch" "$fixture_binary" LIFIC_VERIFY_PORT="$port" FIXTURE_MODE=full)"
if [[ $status -eq 0 ]]; then
  fail "verifier accepted an occupied port"
fi
echo "an occupied verification port is rejected"

echo "all verify-release-binary.sh checks passed"
