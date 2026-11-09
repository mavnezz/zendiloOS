#!/usr/bin/env bash
# Prints what changed in a build, in Markdown (docs/releases.md). The
# package carries it as /usr/share/zendiloos/changes.md, which Control
# panel → Updates shows under the installed version, without asking
# GitHub; the development build's release on GitHub has it as its notes.
#
#   scripts/release-notes.sh VERSION [CHANGELOG [REPOSITORY]]
#
#   1.2.3, 1.2.3~rc.1        the newest entry of debian/changelog, as bullets
#   1.2.3+dev.<time>.g<hash> "Changes since v1.2.3:" and the subjects of the
#                            commits since that tag, newest first, at most 50
#
# A development build where git cannot tell (no git, no repository, no
# release tag, nothing since it) gets the changelog's entry as well.
# Tested by scripts/release-notes-test.sh (`just release-notes-test`).
set -euo pipefail

here=$(dirname "$0")
version=${1:?usage: release-notes.sh VERSION [CHANGELOG [REPOSITORY]]}
changelog=${2:-$here/../debian/changelog}
repository=${3:-$here/..}
limit=50

# The newest entry of the changelog: each item a bullet on one line, its
# sub-items (- or +) indented below it.
changelog_entry() {
    awk '
        function flush() {
            if (item != "") print item
            item = ""
        }
        NR == 1 { next }
        /^ -- / { exit }
        /^  \* / { flush(); item = "- " substr($0, 5); next }
        /^    [-+] / { flush(); item = "  - " substr($0, 7); next }
        /^ +[^ ]/ { sub(/^ +/, ""); item = (item == "" ? $0 : item " " $0); next }
        END { flush() }
    ' "$changelog"
}

# git as the package build runs it: as root, in a checkout that belongs to
# someone else and is mounted read-only (packaging/build-deb.sh).
in_repository() {
    git -C "$repository" -c safe.directory='*' "$@" 2> /dev/null
}

if [[ $version == *+dev.* && -e $repository/.git ]] && command -v git > /dev/null &&
    last=$(in_repository describe --tags --abbrev=0 --match 'v[0-9]*' HEAD) &&
    total=$(in_repository rev-list --no-merges --count "$last..HEAD") && ((total > 0)); then
    printf 'Changes since %s:\n\n' "$last"
    in_repository log --no-merges --format='- %s' -n "$limit" "$last..HEAD"
    ((total <= limit)) || printf -- '- … and %d earlier changes\n' "$((total - limit))"
else
    changelog_entry
fi
