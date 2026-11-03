#!/usr/bin/env bash
# Writes DIR/SHA256SUMS (`sha256sum` format, M4-2 P25) over the release assets in DIR,
# after checking that all EXPECTED (default 7: the six target tarballs and, since M4-3a
# P47, `ferrite-plugins-<VERSION>.tar.gz`) of them are there.
#
#   .github/scripts/sums.sh DIR [EXPECTED]
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 DIR [EXPECTED]" >&2
  exit 1
fi
dir=$1
expected=${2:-7}
if ! [[ "$expected" =~ ^[0-9]+$ ]]; then
  echo "usage: $0 DIR [EXPECTED]: EXPECTED must be a number" >&2
  exit 1
fi
cd "$dir"
count=$(find . -maxdepth 1 -name 'ferrite-*.tar.gz' | wc -l | tr -d ' ')
if [ "$count" -ne "$expected" ]; then
  ls -l
  echo "::error::expected $expected assets, found $count"
  exit 1
fi
# M6-2: the signed plugins' registry assets and the registry index, when the release has
# them (the plugin keys are set), are listed too.
shopt -s nullglob
files=(ferrite-*.tar.gz ferrite-plugin-*.toml ferrite-plugin-*.wasm)
shopt -u nullglob
# nullglob drops only patterns; fixed names are listed only when they exist.
for f in ferrite-registry-index.json ferrite-registry-index.json.minisig; do
  if [ -e "$f" ]; then
    files+=("$f")
  fi
done
if command -v sha256sum >/dev/null; then
  sha256sum "${files[@]}" > SHA256SUMS
else
  shasum -a 256 "${files[@]}" > SHA256SUMS
fi
cat SHA256SUMS
