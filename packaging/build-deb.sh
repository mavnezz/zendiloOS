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
dpkg-buildpackage --build=binary --no-sign

install -m 0644 /build/*.deb /build/*.buildinfo /build/*.changes /out/
chown "${HOST_UID:-0}:${HOST_GID:-0}" /out/*
