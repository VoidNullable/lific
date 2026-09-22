#!/usr/bin/env bash
# Smoke-test a release binary: it starts on its own, serves the API, and
# actually carries the built web UI inside it.
#
# A partial bundle can return index.html while its missing assets also return
# HTML through the SPA fallback. Check the bundles as well as the document,
# from a scratch directory with an explicit config to exclude caller state.
set -euo pipefail

binary_arg="${1:?usage: verify-release-binary.sh PATH_TO_BINARY}"

# Resolve before changing directory, and without `readlink -f`, which is a GNU
# extension that macOS does not have.
binary_dir="$(cd -- "$(dirname -- "$binary_arg")" >/dev/null 2>&1 && pwd)" || {
  echo "no such directory for binary '$binary_arg'" >&2
  exit 1
}
binary="$binary_dir/$(basename -- "$binary_arg")"
if [[ ! -x $binary ]]; then
  echo "'$binary' is not an executable file" >&2
  exit 1
fi

scratch="$(mktemp -d)"
server_pid=""
cleanup() {
  if [[ -n $server_pid ]]; then
    kill "$server_pid" 2>/dev/null || true
    for _ in {1..50}; do
      if ! kill -0 "$server_pid" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "$server_pid" 2>/dev/null; then
      kill -KILL "$server_pid" 2>/dev/null || true
    fi
    wait "$server_pid" 2>/dev/null || true
  fi
  cd / || true
  rm -rf "$scratch"
}
trap cleanup EXIT

# Everything below runs here, so a binary that reads web/dist, migrations, or a
# lific.toml out of the caller's working directory fails instead of passing on
# borrowed files.
cd "$scratch"

"$binary" --version
"$binary" --help >/dev/null

port="${LIFIC_VERIFY_PORT:-}"

# An override is useful for CI and local debugging. For the automatic case,
# retry a collision so an unrelated local server does not make this test flaky.
if [[ -n $port ]]; then
  if curl --connect-timeout 1 --max-time 2 --silent --output /dev/null "http://127.0.0.1:$port/"; then
    echo "verification port $port is already serving HTTP" >&2
    exit 1
  fi
else
  for _ in {1..20}; do
    candidate="$((34567 + RANDOM % 1000))"
    if ! curl --connect-timeout 1 --max-time 2 --silent --output /dev/null "http://127.0.0.1:$candidate/"; then
      port="$candidate"
      break
    fi
  done
  if [[ -z $port ]]; then
    echo "could not find an available verification port" >&2
    exit 1
  fi
fi

config="$scratch/verify-lific.toml"
cat >"$config" <<TOML
# Written by verify-release-binary.sh. Passed with --config so the run cannot
# inherit a project-local, user, or system lific.toml.
[server]
host = "127.0.0.1"
port = $port

[database]
path = "$scratch/lific.db"

[backup]
enabled = false
dir = "$scratch/backups"

[auth]
required = true
allow_signup = false

[log]
level = "info"
TOML

LIFIC_INIT_ADMIN_NAME=Release \
  LIFIC_INIT_ADMIN_PASSWORD=release-smoke-password-123 \
  "$binary" \
  --config "$config" \
  start --init-if-missing --host 127.0.0.1 --port "$port" \
  >"$scratch/server.log" 2>&1 &
server_pid=$!

startup_timeout="${LIFIC_VERIFY_STARTUP_TIMEOUT:-60}"
if ! [[ $startup_timeout =~ ^[1-9][0-9]*$ ]]; then
  echo "LIFIC_VERIFY_STARTUP_TIMEOUT must be a positive integer" >&2
  exit 1
fi
startup_deadline=$((SECONDS + startup_timeout))

# Seconds left before the startup deadline, clamped to a sane per-request
# budget. Never returns 0: `curl --max-time 0` means "no timeout", so a
# deadline that rolls over mid-loop used to turn the bounded wait into an
# unbounded one.
request_budget() {
  local remaining=$((startup_deadline - SECONDS))
  if ((remaining <= 0)); then
    return 1
  fi
  if ((remaining > 5)); then
    remaining=5
  fi
  printf '%s\n' "$remaining"
}

started=false
healthy=false
while ((SECONDS < startup_deadline)); do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    cat "$scratch/server.log" >&2
    echo "server exited before it served a request" >&2
    exit 1
  fi

  # The production server logs this only after TcpListener::bind succeeds.
  # Waiting for that event ties the HTTP probe to this process rather than
  # accepting the first response from an unrelated listener.
  if grep -q 'lific server started' "$scratch/server.log"; then
    started=true
    if budget="$(request_budget)"; then
      if curl --fail --silent --show-error \
        --connect-timeout 1 --max-time "$budget" \
        "http://127.0.0.1:$port/api/health" >"$scratch/health.txt"; then
        healthy=true
        break
      fi
    fi
  fi
  sleep 1
done

if [[ $started != true || $healthy != true ]] || ! kill -0 "$server_pid" 2>/dev/null; then
  cat "$scratch/server.log" >&2
  echo "server did not report a healthy start within ${startup_timeout}s" >&2
  exit 1
fi

if [[ ! -s $scratch/health.txt ]]; then
  echo "/api/health returned an empty body" >&2
  exit 1
fi

if ! curl --fail --silent --show-error --connect-timeout 2 --max-time 15 \
  "http://127.0.0.1:$port/" >"$scratch/index.html"; then
  cat "$scratch/server.log" >&2
  echo "the server did not serve its web root" >&2
  exit 1
fi

if [[ ! -s $scratch/index.html ]]; then
  echo "the web root is empty" >&2
  exit 1
fi
if ! grep -qi '<html' "$scratch/index.html"; then
  echo "the web root is not an HTML document" >&2
  exit 1
fi

# The shell only proves the frontend shipped if the bundles it names are
# really in the binary, so collect the root-relative asset URLs it points at.
# Vite writes exactly these: /assets/<name>-<hash>.js and .css.
assets="$(grep -Eo '/assets/[A-Za-z0-9._~%+-]+\.(js|css)' "$scratch/index.html" | sort -u || true)"
js_assets="$(printf '%s\n' "$assets" | grep -E '\.js$' | head -n 3 || true)"
css_assets="$(printf '%s\n' "$assets" | grep -E '\.css$' | head -n 3 || true)"

if [[ -z ${js_assets//[[:space:]]/} || -z ${css_assets//[[:space:]]/} ]]; then
  echo "the web root names no /assets JavaScript and stylesheet pair, so this" >&2
  echo "binary was built without the web UI (or with a placeholder shell)" >&2
  sed -n '1,40p' "$scratch/index.html" >&2
  exit 1
fi

# Fetch one asset and prove it is the real file: a 200 alone means nothing
# here, because an asset the binary does not carry falls through to the SPA
# handler, which answers 200 text/html with index.html.
check_asset() {
  local path="$1" kind="$2" body="$scratch/asset-body" content_type
  if ! content_type="$(curl --fail --silent --show-error \
    --connect-timeout 2 --max-time 15 \
    --output "$body" --write-out '%{content_type}' \
    "http://127.0.0.1:$port$path")"; then
    echo "$kind asset $path was not served" >&2
    return 1
  fi
  if [[ ! -s $body ]]; then
    echo "$kind asset $path is empty" >&2
    return 1
  fi

  content_type="$(printf '%s' "$content_type" | tr '[:upper:]' '[:lower:]')"
  case "$kind:$content_type" in
    js:*javascript* | js:*ecmascript*) ;;
    css:*css*) ;;
    *)
      echo "$kind asset $path came back as '$content_type', which means the" >&2
      echo "binary does not contain it and served the SPA fallback instead" >&2
      return 1
      ;;
  esac

  if head -c 200 "$body" | grep -qi '<!doctype html\|<html'; then
    echo "$kind asset $path is an HTML document, not a bundle" >&2
    return 1
  fi
}

while read -r asset; do
  [[ -n $asset ]] || continue
  check_asset "$asset" js
done <<<"$js_assets"

while read -r asset; do
  [[ -n $asset ]] || continue
  check_asset "$asset" css
done <<<"$css_assets"

echo "release binary serves its API and the embedded web UI"
