#!/usr/bin/env bash
# Prints the signing payload of a plugin folder (M6-2, design 14 §2.1): the bytes a
# publisher signs with `minisign -S`, the same as `ferrite plugin digest --dir DIR`.
#
#   .github/scripts/plugin-payload.sh DIR > payload
#   minisign -S -s plugins.key -m payload -x DIR/ferrite-plugin.minisig
#
# DIR holds `ferrite-plugin.toml` and exactly one `*.wasm`.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 DIR" >&2
  exit 1
fi
dir=$1
manifest="$dir/ferrite-plugin.toml"
if [ ! -f "$manifest" ]; then
  echo "$dir has no ferrite-plugin.toml" >&2
  exit 1
fi
shopt -s nullglob
wasm=("$dir"/*.wasm)
if [ "${#wasm[@]}" -ne 1 ]; then
  echo "$dir must hold exactly one .wasm file (found ${#wasm[@]})" >&2
  exit 1
fi

sha() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

printf 'ferrite-plugin-signature/1\nmanifest %s\ncomponent %s\n' \
  "$(sha "$manifest")" "$(sha "${wasm[0]}")"
