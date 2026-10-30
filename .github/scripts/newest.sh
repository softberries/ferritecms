#!/usr/bin/env bash
# Whether VERSION (X.Y.Z) is the newest stable release, overall and within its X.Y line,
# among the repository's `vX.Y.Z` tags (VERSION itself counts even when it is not tagged
# yet). Prints `latest=true|false` and `minor_latest=true|false` (GITHUB_OUTPUT lines).
# A backport (v0.1.5 after v0.2.0) is then not "latest", so it moves neither GitHub's
# latest release (what `ferrite self-update` installs) nor the image's `latest` tag;
# it still moves `0.1`. A prerelease is never either.
#
#   .github/scripts/newest.sh VERSION
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 VERSION" >&2
  exit 1
fi
version=$1
if [[ "$version" == *-* ]]; then
  echo "latest=false"
  echo "minor_latest=false"
  exit 0
fi
minor=${version%.*}
stable=$( { git tag --list 'v*' | sed 's/^v//'; echo "$version"; } \
  | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -uV)
newest=$(printf '%s\n' "$stable" | tail -n 1)
newest_minor=$(printf '%s\n' "$stable" | grep -E "^${minor//./\\.}\.[0-9]+$" | tail -n 1)
if [ "$newest" = "$version" ]; then echo "latest=true"; else echo "latest=false"; fi
if [ "$newest_minor" = "$version" ]; then echo "minor_latest=true"; else echo "minor_latest=false"; fi
