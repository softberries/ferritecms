#!/usr/bin/env bash
# Packs the plugins release asset (M4-3a P47): `ferrite-plugins-<VERSION>.tar.gz` in
# OUT_DIR, holding three folders at its top level, one per example plugin:
#
#   seo/ analytics/ contact-form/   each with ferrite-plugin.toml, <name>.wasm, README.md
#
# so that `ferrite plugin check|install --dir <folder>` works on an extracted folder as it
# is (the plugin READMEs' Install sections describe this layout). The wasm files are the
# release builds of the plugins workspace (`plugins/Cargo.toml`, target wasm32-wasip2) in
# WASM_DIR (default plugins/target/wasm32-wasip2/release), named by crate (`contact_form.wasm`)
# there and `<plugin>.wasm` in the asset.
#
# The plugin version is the workspace version at release: the script refuses when a
# manifest's `version` is not VERSION, so releasing a new version means bumping the three
# manifests together with the workspace version (docs/design/09-operations.md §7).
#
#   .github/scripts/package-plugins.sh VERSION OUT_DIR
#
# Run from the repository root. Used by .github/workflows/release.yml and the CI
# `packaging` job; runs on Linux and macOS (bsdtar).
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 VERSION OUT_DIR" >&2
  exit 1
fi
version=$1
out=$2
wasm_dir=${WASM_DIR:-plugins/target/wasm32-wasip2/release}
name="ferrite-plugins-${version}"

# Each entry: plugin folder in the asset (= the manifest's name) and its crate's wasm file.
plugins=("seo:seo.wasm" "analytics:analytics.wasm" "contact-form:contact_form.wasm")

# The `version` of the manifest's [plugin] table (the first table; other tables may have
# keys of the same name).
manifest_version() {
  awk '
    /^[[:space:]]*\[/ { in_plugin = ($0 ~ /^[[:space:]]*\[plugin\][[:space:]]*(#.*)?$/); next }
    in_plugin && /^[[:space:]]*version[[:space:]]*=/ {
      v = $0
      sub(/^[^=]*=[[:space:]]*"/, "", v)
      sub(/".*$/, "", v)
      print v
      exit
    }
  ' "$1"
}

stage=$(mktemp -d)
trap 'rm -r "$stage"' EXIT

for entry in "${plugins[@]}"; do
  plugin=${entry%%:*}
  wasm=${entry#*:}
  src="plugins/examples/$plugin"
  for f in "$src/ferrite-plugin.toml" "$src/README.md" "$wasm_dir/$wasm"; do
    if [ ! -f "$f" ]; then
      echo "::error::$f is missing" >&2
      exit 1
    fi
  done
  found=$(manifest_version "$src/ferrite-plugin.toml")
  if [ "$found" != "$version" ]; then
    echo "::error::$src/ferrite-plugin.toml has version '$found', expected '$version': bump the plugin manifests with the workspace version" >&2
    exit 1
  fi
  mkdir "$stage/$plugin"
  cp "$src/ferrite-plugin.toml" "$src/README.md" "$stage/$plugin/"
  cp "$wasm_dir/$wasm" "$stage/$plugin/$plugin.wasm"
  chmod 0644 "$stage/$plugin"/*
done

mkdir -p "$out"
# COPYFILE_DISABLE keeps macOS tar from adding `._*` AppleDouble entries.
COPYFILE_DISABLE=1 tar -C "$stage" -czf "$out/$name.tar.gz" "${plugins[@]%%:*}"
echo "packed $out/$name.tar.gz:"
tar -tzvf "$out/$name.tar.gz"
