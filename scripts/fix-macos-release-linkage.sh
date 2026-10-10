#!/usr/bin/env bash
set -euo pipefail

binary="${1:?usage: fix-macos-release-linkage.sh <binary>}"

# Nix's Darwin linker can select its own libiconv. A release binary must use
# the copy supplied by macOS, since users do not have the CI runner's store.
dependencies="$(/usr/bin/otool -L "$binary")"
nix_iconv="$(printf '%s\n' "$dependencies" | awk '$1 ~ /^\/nix\/store\/.*\/libiconv\.2\.dylib$/ { print $1 }')"

if [[ -n "$nix_iconv" ]]; then
  /usr/bin/install_name_tool -change "$nix_iconv" /usr/lib/libiconv.2.dylib "$binary"
  # Changing a Mach-O load command invalidates the linker's ad hoc signature.
  /usr/bin/codesign --force --sign - "$binary"
  /usr/bin/codesign --verify "$binary"
fi

dependencies="$(/usr/bin/otool -L "$binary")"
if [[ "$dependencies" == *"/nix/store/"* ]]; then
  printf 'macOS release binary still references a Nix store library:\n%s\n' "$dependencies" >&2
  exit 1
fi
