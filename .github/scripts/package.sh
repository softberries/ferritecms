#!/usr/bin/env bash
# Packs one release asset (M4-2 P25): `ferrite-<VERSION>-<TARGET>.tar.gz` in OUT_DIR,
# holding the folder `ferrite-<VERSION>-<TARGET>/` with `ferrite` (mode 0755), README.md and
# LICENSE* (when present). `ferrite self-update` accepts the binary at the top level or one
# folder down, so this layout is what it reads.
#
#   .github/scripts/package.sh VERSION TARGET BINARY OUT_DIR
#
# Run from the repository root. Used by .github/workflows/release.yml; runs on Linux (GNU
# tar) and macOS (bsdtar). The archive is reproducible (M4-4 P83, repro-tar.sh): the same
# files give the same bytes. The binary itself is not claimed to be.
set -euo pipefail

# shellcheck source=repro-tar.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/repro-tar.sh"

if [ "$#" -ne 4 ]; then
  echo "usage: $0 VERSION TARGET BINARY OUT_DIR" >&2
  exit 1
fi
version=$1
target=$2
binary=$3
out=$4
name="ferrite-${version}-${target}"

stage=$(mktemp -d)
trap 'rm -r "$stage"' EXIT
mkdir "$stage/$name"
cp "$binary" "$stage/$name/ferrite"
chmod 0755 "$stage/$name/ferrite"
cp README.md "$stage/$name/"
for f in LICENSE*; do
  if [ -f "$f" ]; then cp "$f" "$stage/$name/"; fi
done
# Modes do not depend on the source files' or the umask.
find "$stage/$name" -type f ! -name ferrite -exec chmod 0644 {} +

mkdir -p "$out"
repro_pack "$stage" "$out/$name.tar.gz" "$name"
echo "packed $out/$name.tar.gz:"
tar -tzvf "$out/$name.tar.gz"
