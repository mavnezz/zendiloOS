#!/usr/bin/env bash
# Checks scripts/release-notes.sh in a repository of its own: a release
# gets the changelog's newest entry, a development build the commits since
# the last release tag, and without git the changelog's entry again.
# `just release-notes-test`; part of `just check`.
set -euo pipefail

notes=$(cd "$(dirname "$0")" && pwd)/release-notes.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo=$work/repo
changelog=$repo/debian/changelog
dev=0.2.0+dev.20261109120000.gabc1234

# Nothing of this machine's git configuration (signing, hooks, the default
# branch), and the same author everywhere.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.org
export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.org

failed=0
# expect WHAT EXPECTED ACTUAL
expect() {
    if [[ $2 == "$3" ]]; then
        echo "ok    $1"
    else
        echo "FAIL  $1"
        diff -u <(printf '%s\n' "$2") <(printf '%s\n' "$3") | tail -n +3 || true
        failed=1
    fi
}
# One commit a minute, so that their order is the same on every run.
at=1793000000
tick() {
    at=$((at + 60))
    export GIT_AUTHOR_DATE="$at +0000" GIT_COMMITTER_DATE="$at +0000"
}
commit() {
    tick
    git -C "$repo" commit -q --allow-empty -m "$1"
}

mkdir -p "$repo/debian"
cat > "$changelog" << 'EOF'
zendiloos (0.2.0) unstable; urgency=medium

  * A file explorer replaces the Shares app: folders on a volume with their
    own permissions, shared over SMB and NFS when asked.
  * Plugins, installed from the App Center:
    - Docker
    - an ACME client for Let's Encrypt
  * Two-factor sign-in.

 -- zendiloOS <github.com@stuch.me>  Sun, 08 Nov 2026 11:13:07 +0000

zendiloos (0.1.0) unstable; urgency=medium

  * Initial package.

 -- zendiloOS <github.com@stuch.me>  Thu, 05 Nov 2026 12:00:00 +0100
EOF
entry='- A file explorer replaces the Shares app: folders on a volume with their own permissions, shared over SMB and NFS when asked.
- Plugins, installed from the App Center:
  - Docker
  - an ACME client for Let'"'"'s Encrypt
- Two-factor sign-in.'

expect 'a release gets the newest changelog entry' "$entry" "$("$notes" 0.2.0 "$changelog" "$repo")"
expect 'so does a test release' "$entry" "$("$notes" 0.2.0~rc.1 "$changelog" "$repo")"
expect 'a development build outside a repository too' "$entry" "$("$notes" "$dev" "$changelog" "$repo")"

git -C "$repo" init -q -b main
git -C "$repo" add debian/changelog
commit 'Release 0.1.0'
git -C "$repo" tag v0.1.0
expect 'a development build of the tag itself too' "$entry" "$("$notes" "$dev" "$changelog" "$repo")"

commit 'Show the installed version'"'"'s notes'
git -C "$repo" switch -q -c side
commit 'Rename the update page'
git -C "$repo" switch -q main
tick
git -C "$repo" merge -q --no-ff -m 'Merge branch side' side
commit 'Render notes as Markdown'
# Neither the rolling release's tag nor others count as a release.
git -C "$repo" tag dev
git -C "$repo" tag build-1
expect 'a development build lists the commits since the last release, no merges' "Changes since v0.1.0:

- Render notes as Markdown
- Rename the update page
- Show the installed version's notes" "$("$notes" "$dev" "$changelog" "$repo")"
expect 'a release still gets the changelog' "$entry" "$("$notes" 0.2.0 "$changelog" "$repo")"

# Without git, the changelog's entry again.
mkdir "$work/bin"
ln -s "$(command -v awk)" "$(command -v dirname)" "$work/bin/"
expect 'a development build without git gets the changelog' "$entry" \
    "$(PATH=$work/bin "$BASH" "$notes" "$dev" "$changelog" "$repo")"

for i in $(seq 1 50); do commit "Change $i"; done
expected=$(
    printf 'Changes since v0.1.0:\n\n'
    for i in $(seq 50 -1 1); do echo "- Change $i"; done
    echo '- … and 3 earlier changes'
)
expect 'at most 50, and how many more' "$expected" "$("$notes" "$dev" "$changelog" "$repo")"

git -C "$repo" tag v0.2.0
commit 'After the release'
expect 'counted from the newest release tag' "Changes since v0.2.0:

- After the release" "$("$notes" 0.2.0+dev.1.g1 "$changelog" "$repo")"

exit "$failed"
