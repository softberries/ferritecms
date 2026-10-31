#!/usr/bin/env bash
# Checks that package.sh and package-plugins.sh give reproducible archives (M4-4 P83):
# each is packed twice, from stage folders at other paths and with source files of other
# mtimes, and the two archives must be byte-identical; then the archive's members are read
# back: sorted names with no `./`, owner 0 with no names, mtime 0, ustar, modes 0755 for
# folders and `ferrite`, 0644 for the rest, and no gzip header timestamp.
#
#   .github/scripts/test-repro.sh
#
# Run from the repository root, on Linux (GNU tar) and macOS (bsdtar). SCRIPT_DIR (default
# .github/scripts) names the folder of the scripts under test. The CI `packaging` job runs
# it; it needs bash, tar, gzip, python3 and a README.md.
set -euo pipefail

script_dir=${SCRIPT_DIR:-.github/scripts}
# package-plugins.sh refuses a manifest of another version, so this is the workspace's.
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
target=x86_64-unknown-linux-gnu

work=$(mktemp -d)
trap 'rm -r "$work"' EXIT

sha() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# A fake binary and fake wasm files, with the mtimes the second pack changes.
mkdir "$work/src" "$work/wasm"
printf '#!/bin/sh\necho fake\n' >"$work/src/ferrite"
chmod 0700 "$work/src/ferrite"
for w in seo analytics contact_form; do
  printf 'fake wasm %s\n' "$w" >"$work/wasm/$w.wasm"
done

pack_both() {
  local out=$1
  "$script_dir/package.sh" "$version" "$target" "$work/src/ferrite" "$out" >/dev/null
  WASM_DIR="$work/wasm" "$script_dir/package-plugins.sh" "$version" "$out" >/dev/null
}

pack_both "$work/one"
# Other mtimes on the inputs, another umask and a pause (the scripts copy the files, so
# a copy time or a mode left to the umask would show).
touch -t 200001010000.00 "$work/src/ferrite" "$work/wasm"/*.wasm
sleep 2
(umask 077 && pack_both "$work/two")

status=0
for asset in "ferrite-$version-$target" "ferrite-plugins-$version"; do
  a=$(sha "$work/one/$asset.tar.gz")
  b=$(sha "$work/two/$asset.tar.gz")
  if [ "$a" = "$b" ]; then
    echo "ok    same sha256 twice: $asset ($a)"
  else
    echo "FAIL  $asset differs between two packs: $a $b" >&2
    status=1
  fi
done

if ! python3 -I - "$work/one" "$version" "$target" <<'EOF'
import gzip, sys, tarfile

out, version, target = sys.argv[1:4]
bad = []
for asset in (f"ferrite-{version}-{target}", f"ferrite-plugins-{version}"):
    path = f"{out}/{asset}.tar.gz"
    raw = open(path, "rb").read()
    # gzip header: magic, method, flags (no FNAME), 4 bytes of mtime (zero).
    if raw[:4] != b"\x1f\x8b\x08\x00" or raw[4:8] != b"\x00\x00\x00\x00":
        bad.append(f"{asset}: the gzip header has a name or a timestamp")
    # ustar: the first header's magic is "ustar" NUL and the version "00" (GNU is "ustar  ").
    if gzip.decompress(raw)[257:265] != b"ustar\x0000":
        bad.append(f"{asset}: the first header is not ustar")
    with tarfile.open(path) as tar:
        members = tar.getmembers()
        names = [m.name for m in members]
    if names != sorted(names, key=lambda n: n.encode()):
        bad.append(f"{asset}: member names are not sorted: {names}")
    if any(n.startswith(("./", "/")) for n in names):
        bad.append(f"{asset}: a name starts with ./ or /: {names}")
    for m in members:
        if (m.uid, m.gid, m.uname, m.gname) != (0, 0, "", ""):
            bad.append(f"{asset}: {m.name} owner {m.uid}:{m.gid} {m.uname!r}:{m.gname!r}")
        if m.mtime != 0:
            bad.append(f"{asset}: {m.name} mtime {m.mtime}")
        want = 0o755 if m.isdir() or m.name.endswith("/ferrite") else 0o644
        if m.mode != want:
            bad.append(f"{asset}: {m.name} mode {m.mode:o}, want {want:o}")
        if not (m.isdir() or m.isreg()):
            bad.append(f"{asset}: {m.name} is neither a folder nor a regular file")
    print(f"ok    {asset}: {len(members)} members")
if bad:
    print("\n".join("FAIL  " + b for b in bad), file=sys.stderr)
    sys.exit(1)
EOF
then
  status=1
fi
exit $status
