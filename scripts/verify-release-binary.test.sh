#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
occupied_pid=""
cleanup() {
  if [[ -n $occupied_pid ]]; then
    kill "$occupied_pid" 2>/dev/null || true
    wait "$occupied_pid" 2>/dev/null || true
  fi
  rm -rf "$scratch"
}
trap cleanup EXIT

fake_binary="$scratch/failing-lific"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'case " $* " in' \
  '  *" --version "*) echo "lific test" ;;' \
  '  *" --help "*) exit 0 ;;' \
  '  *" start "*) exit 1 ;;' \
  '  *) exit 0 ;;' \
  'esac' >"$fake_binary"
chmod +x "$fake_binary"

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

if LIFIC_VERIFY_PORT="$port" bash "$script_dir/verify-release-binary.sh" "$fake_binary"; then
  echo "verifier accepted an occupied port" >&2
  exit 1
fi

echo "occupied port with failing child is rejected"

no_response_server="$scratch/no-response-server.ts"
printf '%s\n' \
  'Bun.serve({ port: Number(process.env.PORT), fetch: () => new Promise(() => {}) });' \
  'console.error("lific server started");' \
  'console.error("no-response fixture listening");' \
  >"$no_response_server"
no_response_binary="$scratch/no-response-lific"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'case " $* " in' \
  '  *" --version "*) echo "lific test" ;;' \
  '  *" --help "*) exit 0 ;;' \
  '  *" start "*) echo "lific server started" >&2; exec env PORT="${LIFIC_VERIFY_PORT:?}" bun "$NO_RESPONSE_SERVER" ;;' \
  '  *) exit 0 ;;' \
  'esac' >"$no_response_binary"
chmod +x "$no_response_binary"

hang_port="$(free_port)"
timeout_command=timeout
if ! command -v "$timeout_command" >/dev/null 2>&1; then
  timeout_command=gtimeout
fi
if ! command -v "$timeout_command" >/dev/null 2>&1; then
  echo "could not find a timeout command for the non-responding fixture" >&2
  exit 1
fi

set +e
LIFIC_VERIFY_PORT="$hang_port" \
  LIFIC_VERIFY_STARTUP_TIMEOUT=3 \
  NO_RESPONSE_SERVER="$no_response_server" \
  "$timeout_command" 10 bash "$script_dir/verify-release-binary.sh" "$no_response_binary" \
  >"$scratch/no-response.log" 2>&1
status=$?
set -e

if [[ $status -eq 124 ]]; then
  cat "$scratch/no-response.log" >&2
  echo "verifier hung on a non-responding server" >&2
  exit 1
fi
if [[ $status -eq 0 ]]; then
  echo "verifier accepted a non-responding server" >&2
  exit 1
fi
if ! grep -Fq "no-response fixture listening" "$scratch/no-response.log"; then
  cat "$scratch/no-response.log" >&2
  echo "non-responding fixture did not bind its port" >&2
  exit 1
fi

echo "non-responding server is rejected within the startup budget"
