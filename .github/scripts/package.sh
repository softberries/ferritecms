#!/usr/bin/env bash
# Packs one release asset (M4-2 P25): `ferrite-<VERSION>-<TARGET>.tar.gz` in OUT_DIR,
# holding the folder `ferrite-<VERSION>-<TARGET>/` with `ferrite` (mode 0755), README.md and
# LICENSE* (when present). `ferrite self-update` accepts the binary at the top level or one
# folder down, so this layout is what it reads.
#
#   .github/scripts/package.sh VERSION TARGET BINARY OUT_DIR
#
# Run from the repository root. Used by .github/workflows/release.yml; runs on Linux and
# macOS (bsdtar).
set -euo pipefail

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

mkdir -p "$out"
# COPYFILE_DISABLE keeps macOS tar from adding `._*` AppleDouble entries.
COPYFILE_DISABLE=1 tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
echo "packed $out/$name.tar.gz:"
tar -tzvf "$out/$name.tar.gz"
