#!/usr/bin/env bash
# Builds the Debian package inside the build container; see `just deb`.
# The repository is mounted read-only at /src, the output directory at /out.
set -euo pipefail

# Build in a copy; target/ and the npm cache live in volumes between builds.
rsync -a --delete \
    --exclude=.git/ --exclude=dev/ --exclude=dist/ \
    --exclude=backend/target/ --exclude=frontend/node_modules/ --exclude=frontend/dist/ \
    /src/ /build/zendiloos/

cd /build/zendiloos
# A release build carries its version (scripts/release-version.sh); others
# keep the changelog's.
if [[ -n ${ZENDILOOS_VERSION:-} ]]; then
    [[ $ZENDILOOS_VERSION =~ ^[0-9][0-9A-Za-z.+~]*$ ]] || { echo "not a version: $ZENDILOOS_VERSION" >&2; exit 1; }
    sed -i "1s/^zendiloos ([^)]*)/zendiloos ($ZENDILOOS_VERSION)/" debian/changelog
fi
# A development build's notes list the commits since the last release tag,
# read from the repository itself (scripts/release-notes.sh).
ZENDILOOS_REPOSITORY=/src dpkg-buildpackage --build=binary --no-sign

install -m 0644 /build/*.deb /build/*.buildinfo /build/*.changes /out/
chown "${HOST_UID:-0}:${HOST_GID:-0}" /out/*
