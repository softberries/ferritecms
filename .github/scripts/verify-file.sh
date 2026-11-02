#!/usr/bin/env bash
# Checks FILE.minisig (or SIG) over FILE with KEY_FILE (M6-2): a minisign public key file,
# optionally with `#` comment lines (as crates/ferrite/registry.pub), or the bare base64
# key; the signature must be prehashed ("ED"). Exits 3 when KEY_FILE holds no key (a
# placeholder), so callers can skip with a notice.
#
#   .github/scripts/verify-file.sh KEY_FILE FILE [SIG]
set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 KEY_FILE FILE [SIG]" >&2
  exit 1
fi
key_file=$1
file=$2
sig=${3:-$file.minisig}
pub=$(mktemp)
trap 'rm -f "$pub"' EXIT
grep -v '^[[:space:]]*#' "$key_file" | grep '[^[:space:]]' > "$pub" || true
case "$(wc -l < "$pub" | tr -d ' ')" in
  0)
    echo "::notice::$key_file holds no key (a placeholder)"
    exit 3
    ;;
  1) minisign -V -q -P "$(cat "$pub")" -m "$file" -x "$sig" ;;
  2) minisign -V -q -p "$pub" -m "$file" -x "$sig" ;;
  *)
    echo "::error::$key_file is not a minisign public key"
    exit 1
    ;;
esac
alg=$(sed -n 2p "$sig" | base64 -d | head -c 2)
if [ "$alg" != ED ]; then
  echo "::error::the signature is not prehashed (algorithm '$alg')"
  exit 1
fi
