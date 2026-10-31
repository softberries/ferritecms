#!/usr/bin/env bash
# Builds the FerriteCMS documentation site as a local FerriteCMS site (M4-3b P63):
# a fresh data folder, an admin account, `ferrite import markdown site/content
# --front-page` and the `docs` theme. Usage:
#
#   site/build.sh OUT_DIR [URL]
#
# OUT_DIR must not hold a site yet (`ferrite init` refuses an existing one). URL is the
# site's public URL (default http://localhost:8080). FERRITE is the command that runs
# ferrite (default `cargo run -q -p ferrite --`, run from the repository root); it is split
# on whitespace, so a binary path holding spaces needs a symlink or a PATH entry instead.
#
# The admin account `docs` is created without printing its one-time password; set one
# with `ferrite user reset-password --data OUT_DIR --login docs` to sign in.
#
# Exits non-zero when any step fails, including an import that exits 2 (a skipped file,
# a missing image or a broken link is a build failure).
set -euo pipefail

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: site/build.sh OUT_DIR [URL]" >&2
  exit 64
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$1"
case "$out" in
  /*) ;;
  *) out="$PWD/$out" ;;
esac
url="${2:-http://localhost:8080}"
read -r -a ferrite <<< "${FERRITE:-cargo run -q -p ferrite --}"

cd "$root"

echo "==> init $out ($url)"
"${ferrite[@]}" init --data "$out" --url "$url" --title "FerriteCMS"

echo "==> admin account 'docs' (no password shown; see the README to set one)"
"${ferrite[@]}" user create --data "$out" --login docs --email docs@example.com > /dev/null

echo "==> import markdown site/content"
status=0
"${ferrite[@]}" import markdown "$root/site/content" --data "$out" --front-page \
  --source ferrite-docs || status=$?
if [ "$status" -ne 0 ]; then
  echo "error: ferrite import markdown exited $status (2: skipped files, missing images or broken links, listed above)" >&2
  echo "fix the files, then re-run the import into this folder (site/README.md) or build into a new folder" >&2
  exit "$status"
fi

echo "==> activate the docs theme"
"${ferrite[@]}" theme activate --data "$out" docs

# The address to listen on, for a loopback URL: the URL's port on 127.0.0.1 (`localhost`,
# `127.0.0.1`; nothing to say for the default 127.0.0.1:8080) or on [::1] (always: the
# default listens on IPv4 only). Other hosts get no hint.
listen=""
hostport="${url#*://}"
hostport="${hostport%%/*}"
case "$hostport" in
  "[::1]"*)
    host="[::1]"
    port="${hostport#"[::1]"}"
    port="${port#:}"
    ;;
  *)
    host="${hostport%%:*}"
    port=""
    if [ "$host" != "$hostport" ]; then
      port="${hostport##*:}"
    fi
    ;;
esac
if [ -z "$port" ]; then
  case "$url" in
    https://*) port=443 ;;
    *) port=80 ;;
  esac
fi
case "$host" in
  localhost | 127.0.0.1)
    if [ "$port" != "8080" ]; then
      listen="FERRITE_SERVER__LISTEN=127.0.0.1:$port "
    fi
    ;;
  "[::1]")
    # Quoted: a shell may read the brackets as a glob.
    listen="FERRITE_SERVER__LISTEN='[::1]:$port' "
    ;;
esac

echo
echo "Built. Serve it with:"
printf '  %s%s serve --data %q\n' "$listen" "${FERRITE:-cargo run -q -p ferrite --}" "$out"
