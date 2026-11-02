#!/usr/bin/env bash
# Signs the example plugins and writes the registry index (M6-2, design 14 §11), for
# release.yml's `plugins-sign` (the release keys) and `plugins-sign-dry-run` (throwaway
# keys):
#
#   .github/scripts/sign-plugins.sh VERSION OUT_DIR PLUGINS_KEY PLUGINS_PUB REGISTRY_KEY \
#       REGISTRY_PUB PREVIOUS_INDEX|-
#
# PLUGINS_KEY / REGISTRY_KEY are minisign secret key files (their passwords in
# PLUGINS_PASSWORD / REGISTRY_PASSWORD, empty for a key made with -W); the .pub files are
# their public halves. Each plugin's payload (plugin-payload.sh) is signed into
# `ferrite-plugin.minisig`, `ferrite-plugin.pub` added, the tarball and the per-plugin
# assets packed (package-plugins.sh), the index merged with PREVIOUS_INDEX (already
# verified by the caller; `-` for the first) by registry-index.py and signed as
# `ferrite-registry-index.json(.minisig)`; then every signature is verified with the
# public keys. The wasm comes from WASM_DIR as for package-plugins.sh.
set -euo pipefail

if [ "$#" -ne 7 ]; then
  echo "usage: $0 VERSION OUT_DIR PLUGINS_KEY PLUGINS_PUB REGISTRY_KEY REGISTRY_PUB PREVIOUS|-" >&2
  exit 1
fi
version=$1
out=$2
plugins_key=$3
plugins_pub=$4
registry_key=$5
registry_pub=$6
previous=$7
here=$(dirname "${BASH_SOURCE[0]}")
wasm_dir=${WASM_DIR:-plugins/target/wasm32-wasip2/release}
signed=$(mktemp -d)
trap 'rm -r "$signed"' EXIT

# Keyless keys (-W) read no password; the others read it from stdin.
sign() {
  local key=$1 password=$2 file=$3 sig=$4
  if [ -n "$password" ]; then
    printf '%s\n' "$password" | minisign -S -s "$key" -m "$file" -x "$sig" >/dev/null
  else
    minisign -S -s "$key" -m "$file" -x "$sig" </dev/null >/dev/null
  fi
}

for entry in "seo:seo.wasm" "analytics:analytics.wasm" "contact-form:contact_form.wasm"; do
  plugin=${entry%%:*}
  wasm=${entry#*:}
  mkdir -p "$signed/$plugin"
  cp "plugins/examples/$plugin/ferrite-plugin.toml" "$signed/$plugin/"
  cp "$wasm_dir/$wasm" "$signed/$plugin/$plugin.wasm"
  "$here/plugin-payload.sh" "$signed/$plugin" > "$signed/$plugin.payload"
  sign "$plugins_key" "${PLUGINS_PASSWORD:-}" "$signed/$plugin.payload" \
    "$signed/$plugin/ferrite-plugin.minisig"
  cp "$plugins_pub" "$signed/$plugin/ferrite-plugin.pub"
  "$here/verify-file.sh" "$plugins_pub" "$signed/$plugin.payload" \
    "$signed/$plugin/ferrite-plugin.minisig"
  rm "$signed/$plugin.payload"
done

SIGNATURES_DIR="$signed" ASSETS=1 "$here/package-plugins.sh" "$version" "$out"

python3 "$here/registry-index.py" --previous "$previous" \
  --publishers registry/publishers.json --yanked registry/yanked.json \
  --version "$version" \
  --base-url "https://github.com/${GITHUB_REPOSITORY:-softberries/ferritecms}/releases/download/v$version" \
  --plugins "$signed" --out "$out/ferrite-registry-index.json" \
  ${EXTRA_KEY:+--extra-key "$EXTRA_KEY"}
sign "$registry_key" "${REGISTRY_PASSWORD:-}" "$out/ferrite-registry-index.json" \
  "$out/ferrite-registry-index.json.minisig"
"$here/verify-file.sh" "$registry_pub" "$out/ferrite-registry-index.json"
echo "signed the plugins and the registry index in $out"
