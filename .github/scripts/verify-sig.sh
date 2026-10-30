#!/usr/bin/env bash
# Checks DIR/SHA256SUMS.minisig over DIR/SHA256SUMS with KEY_FILE the way `ferrite
# self-update` does (M4-2 P26): KEY_FILE is a minisign public key file, optionally with
# `#` comment lines (as crates/ferrite/release.pub), or the bare base64 key; the signature
# must be prehashed ("ED", minisign's default), legacy ones are refused.
#
#   .github/scripts/verify-sig.sh KEY_FILE DIR
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 KEY_FILE DIR" >&2
  exit 1
fi
key_file=$1
dir=$2
pub=$(mktemp)
trap 'rm -f "$pub"' EXIT
grep -v '^[[:space:]]*#' "$key_file" | grep '[^[:space:]]' > "$pub" || true
case "$(wc -l < "$pub" | tr -d ' ')" in
  1) minisign -V -P "$(cat "$pub")" -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" ;;
  2) minisign -V -p "$pub" -m "$dir/SHA256SUMS" -x "$dir/SHA256SUMS.minisig" ;;
  *)
    echo "::error::$key_file holds no minisign public key"
    exit 1
    ;;
esac
alg=$(sed -n 2p "$dir/SHA256SUMS.minisig" | base64 -d | head -c 2)
if [ "$alg" != ED ]; then
  echo "::error::the signature is not prehashed (algorithm '$alg')"
  exit 1
fi
echo "signature ok (prehashed)"
