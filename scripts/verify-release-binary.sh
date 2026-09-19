#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: verify-release-binary.sh PATH_TO_BINARY}"
if [[ $binary != /* ]]; then
  binary="./$binary"
fi

"$binary" --version
"$binary" --help >/dev/null

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
  rm -rf "$scratch"
}
trap cleanup EXIT

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

LIFIC_INIT_ADMIN_NAME=Release \
  LIFIC_INIT_ADMIN_PASSWORD=release-smoke-password-123 \
  "$binary" \
  --db "$scratch/lific.db" \
  start --init-if-missing --host 127.0.0.1 --port "$port" \
  >"$scratch/server.log" 2>&1 &
server_pid=$!

startup_timeout="${LIFIC_VERIFY_STARTUP_TIMEOUT:-60}"
if ! [[ $startup_timeout =~ ^[1-9][0-9]*$ ]]; then
  echo "LIFIC_VERIFY_STARTUP_TIMEOUT must be a positive integer" >&2
  exit 1
fi
startup_deadline=$((SECONDS + startup_timeout))
started=false
served=false
while ((SECONDS < startup_deadline)); do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    cat "$scratch/server.log" >&2
    exit 1
  fi

  # The production server logs this only after TcpListener::bind succeeds.
  # Waiting for that event ties the HTTP probe to this process rather than
  # accepting the first response from an unrelated listener.
  if grep -q 'lific server started' "$scratch/server.log"; then
    started=true
    request_timeout=$((startup_deadline - SECONDS))
    if ((request_timeout > 5)); then
      request_timeout=5
    fi
    if curl --fail --silent --show-error \
      --connect-timeout 1 --max-time "$request_timeout" \
      "http://127.0.0.1:$port/" >"$scratch/index.html"; then
      served=true
      break
    fi
  fi
  sleep 1
done

if [[ $started != true || $served != true ]] || ! kill -0 "$server_pid" 2>/dev/null; then
  cat "$scratch/server.log" >&2
  exit 1
fi

test -s "$scratch/index.html"
grep -qi '<html' "$scratch/index.html"
