#!/usr/bin/env bash
# Regression tests for render-homebrew-formula.sh.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
render="$script_dir/render-homebrew-formula.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

hash_for() {
  printf '%064d' "$1"
}

cat >"$scratch/sums.txt" <<EOF
$(hash_for 1)  lific-linux-x86_64
$(hash_for 2)  lific-linux-aarch64
$(hash_for 3) *lific-macos-x86_64
$(hash_for 4)  lific-macos-aarch64
$(hash_for 5)  lific-windows-x86_64.exe
EOF

formula="$(bash "$render" v2.11.1 "$scratch/sums.txt")"
base="https://github.com/VoidNullable/lific/releases/download/v2.11.1"

expect_pair() {
  local asset="$1" hash="$2"
  printf '%s\n' "$formula" | grep -A1 -F "url \"$base/$asset\"" | grep -qF "sha256 \"$hash\"" ||
    fail "$asset is not pinned to $hash"
}
expect_pair lific-linux-x86_64 "$(hash_for 1)"
expect_pair lific-linux-aarch64 "$(hash_for 2)"
expect_pair lific-macos-x86_64 "$(hash_for 3)"
expect_pair lific-macos-aarch64 "$(hash_for 4)"
printf '%s\n' "$formula" | grep -q windows && fail "formula mentions the Windows binary"
# Homebrew would otherwise guess the version from the asset name ("86.64").
printf '%s\n' "$formula" | grep -qxF '  version "2.11.1"' || fail "formula does not pin its version"
# Release assets download without the executable bit.
printf '%s\n' "$formula" | grep -qF 'chmod 0755, bin/"lific"' || fail "formula does not make the binary executable"

if command -v ruby >/dev/null 2>&1; then
  printf '%s\n' "$formula" | ruby -c >/dev/null || fail "formula is not valid Ruby"
fi

grep -v lific-macos-aarch64 "$scratch/sums.txt" >"$scratch/missing.txt"
if bash "$render" 2.11.1 "$scratch/missing.txt" >/dev/null 2>&1; then
  fail "rendered a formula without a macOS aarch64 checksum"
fi

if bash "$render" 2.11 "$scratch/sums.txt" >/dev/null 2>&1; then
  fail "accepted a version that is not MAJOR.MINOR.PATCH"
fi

echo "render-homebrew-formula: ok"
