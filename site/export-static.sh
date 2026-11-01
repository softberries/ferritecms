#!/usr/bin/env bash
# Exports the documentation site as static files for GitHub Pages: builds it with
# site/build.sh into a temporary data folder, serves it on a loopback port, mirrors every
# page with wget and rewrites the local origin to BASE_URL. Usage:
#
#   site/export-static.sh OUT_DIR BASE_URL
#
# OUT_DIR must not exist yet. BASE_URL is where the files will be served, with or without
# a path (https://softberries.github.io/ferritecms): root-relative links get the path in
# front. The search form needs the server (`/?s=`), so the export leaves it out; 404.html
# is the site's own 404 page. FERRITE is the command that runs ferrite, as in build.sh;
# PORT the loopback port to serve on (default 18089). Needs wget, curl and python3.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: site/export-static.sh OUT_DIR BASE_URL" >&2
  exit 64
fi
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$1"
case "$out" in
  /*) ;;
  *) out="$PWD/$out" ;;
esac
base="${2%/}"
port="${PORT:-18089}"
origin="http://127.0.0.1:$port"
if [ -e "$out" ]; then
  echo "error: $out already exists" >&2
  exit 1
fi
read -r -a ferrite <<< "${FERRITE:-cargo run -q -p ferrite --}"

work="$(mktemp -d)"
server=""
cleanup() {
  if [ -n "$server" ]; then
    kill "$server" 2> /dev/null || true
    wait "$server" 2> /dev/null || true
  fi
  rm -r "$work"
}
trap cleanup EXIT

"$root/site/build.sh" "$work/site" "$origin" > "$work/build.log" || {
  cat "$work/build.log" >&2
  exit 1
}

echo "==> serve on $origin"
FERRITE_SERVER__LISTEN="127.0.0.1:$port" "${ferrite[@]}" serve --data "$work/site" \
  > "$work/serve.log" 2>&1 &
server=$!
for _ in $(seq 1 120); do
  if curl -fsS "$origin/readyz" > /dev/null 2>&1; then break; fi
  if ! kill -0 "$server" 2> /dev/null; then
    cat "$work/serve.log" >&2
    exit 1
  fi
  sleep 0.5
done
curl -fsS "$origin/readyz" > /dev/null

echo "==> mirror"
mkdir -p "$out"
# Exit 8 is a 4xx/5xx somewhere: checked below with the link check, not here.
wget --quiet --mirror --page-requisites --no-host-directories --no-parent -e robots=off \
  --directory-prefix "$out" "$origin/" || [ $? -eq 8 ]
for path in robots.txt sitemap.xml; do
  curl -fsS "$origin/$path" -o "$out/$path"
done
# The sitemap index names the sitemaps it is made of.
python3 -I - "$out/sitemap.xml" "$origin" << 'EOF' | while read -r path; do
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()
for loc in re.findall(r"<loc>([^<]+)</loc>", text):
    if loc.startswith(sys.argv[2] + "/") and loc.endswith(".xml"):
        print(loc[len(sys.argv[2]) + 1:])
EOF
  mkdir -p "$out/$(dirname "$path")"
  curl -fsS "$origin/$path" -o "$out/$path"
done
status=$(curl -sS -o "$out/404.html" -w '%{http_code}' "$origin/__no_such_page__/")
if [ "$status" != "404" ]; then
  echo "error: the 404 page answered $status" >&2
  exit 1
fi

echo "==> rewrite links for $base"
python3 -I "$root/site/rewrite-static.py" "$out" "$origin" "$base"
touch "$out/.nojekyll"
echo "Exported to $out"
