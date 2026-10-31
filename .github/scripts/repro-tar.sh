# shellcheck shell=bash
# Sourced by package.sh and package-plugins.sh (M4-4 P83): packs a staged tree into a
# reproducible `.tar.gz`, so the same files give the same bytes whatever the runner, the
# stage path, the umask, the copy times or the user.
#
#   repro_pack STAGE OUT_FILE MEMBER...
#
# MEMBER... are the top-level folders of STAGE to pack. The caller has set the file modes
# (0644 or 0755); the folders get 0755 here. What is fixed, and why:
#   - every mtime is 1970-01-01 00:00 UTC (a `touch` on the stage; bsdtar has no --mtime);
#   - the members are an explicit `LC_ALL=C sort`ed list, not in directory-read order,
#     and `--no-recursion` keeps tar from adding its own;
#   - ustar (no pax headers, whose contents differ between GNU tar and bsdtar), owner and
#     group 0 with no names;
#   - `gzip -n -9`: no name or timestamp in the gzip header (never `tar -z`, whose
#     bsdtar variant writes one).
# The same input gives the same output on the same kind of tar; GNU tar and bsdtar do not
# promise byte-identical output to each other, and each asset is built by one runner type.
repro_pack() (
  stage=$1 out_file=$2
  shift 2

  find "$stage" -type d -exec chmod 0755 {} +
  TZ=UTC find "$stage" -exec touch -h -t 197001010000.00 {} +

  # A subshell function: the trap below does not leak into the caller.
  list=$(mktemp)
  trap 'rm -f "$list"' EXIT
  # NUL-separated, so a name with a space, a newline or a quote is one member. `find` runs
  # inside the stage so the names are relative, with no leading `./`.
  (cd "$stage" && find "$@" -print0 | LC_ALL=C sort -z) >"$list"

  # Captured first: `tar --version | grep -q` under pipefail can fail on SIGPIPE.
  version=$(tar --version)
  if [[ $version == *bsdtar* ]]; then
    owner=(--uid 0 --gid 0 --uname '' --gname '' --numeric-owner)
  else
    owner=(--owner=0 --group=0 --numeric-owner)
  fi
  # COPYFILE_DISABLE keeps macOS tar from adding `._*` AppleDouble entries.
  COPYFILE_DISABLE=1 tar -C "$stage" --no-recursion --format=ustar "${owner[@]}" \
    -cf - --null -T "$list" | gzip -n -9 >"$out_file"
)
