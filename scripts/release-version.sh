#!/usr/bin/env bash
# Prints the Debian version of a build, from git (docs/releases.md):
#
#   tag v1.2.3          → 1.2.3                  stable
#   tag v1.2.3-rc.1     → 1.2.3~rc.1             test (sorts before 1.2.3)
#   a commit on main    → <last tag>+dev.<UTC time>.g<commit>   development
#
# The ref comes as the first argument or from GITHUB_REF; without a release
# tag, the build counts as a development build of the current commit.
set -euo pipefail

ref=${1:-${GITHUB_REF:-}}
to_debian() {
    local tag=${1#v}
    [[ $tag =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "not a release tag: $1" >&2; exit 1; }
    printf '%s\n' "${tag/-/\~}"
}

if [[ $ref == refs/tags/v* ]]; then
    to_debian "${ref#refs/tags/}"
    exit 0
fi

last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
if [[ -n $last ]]; then
    base=$(to_debian "$last")
else
    base=$(sed -n '1s/^zendiloos (\([^)]*\)).*/\1/p' "$(dirname "$0")/../debian/changelog")
fi
printf '%s+dev.%s.g%s\n' "$base" "$(date -u +%Y%m%d%H%M%S)" "$(git rev-parse --short=7 HEAD)"
