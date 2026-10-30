#!/usr/bin/env bash
# Writes DIR/SHA256SUMS (`sha256sum` format, M4-2 P25) over the release assets in DIR,
# after checking that all EXPECTED (default 6) of them are there.
#
#   .github/scripts/sums.sh DIR [EXPECTED]
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 DIR [EXPECTED]" >&2
  exit 1
fi
dir=$1
expected=${2:-6}
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
if command -v sha256sum >/dev/null; then
  sha256sum ferrite-*.tar.gz > SHA256SUMS
else
  shasum -a 256 ferrite-*.tar.gz > SHA256SUMS
fi
cat SHA256SUMS
