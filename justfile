set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Format all code
[working-directory: 'backend']
fmt:
    cargo fmt --all

# Check formatting and lints; fails on any warning
[working-directory: 'backend']
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Run all tests
[working-directory: 'backend']
test:
    cargo test --workspace --locked

# Everything CI checks
check: lint deny test sqlx-check api-check web-check

# The SQL macros check queries against this schema-only database.
sqlx_db := "sqlite://" + justfile_directory() + "/dev/sqlx.db"

# Refresh the offline query data in backend/.sqlx after changing SQL or migrations
[working-directory: 'backend']
sqlx-prepare: sqlx-db
    DATABASE_URL={{ sqlx_db }} cargo sqlx prepare --workspace -- --all-targets

# Fail if backend/.sqlx is out of date
[working-directory: 'backend']
sqlx-check: sqlx-db
    DATABASE_URL={{ sqlx_db }} cargo sqlx prepare --workspace --check -- --all-targets

[private]
[working-directory: 'backend']
sqlx-db:
    mkdir -p ../dev
    DATABASE_URL={{ sqlx_db }} sqlx database reset -y --source crates/zendiloos-infra/migrations

# Run the API server for development; state lives in dev/state
[working-directory: 'backend']
dev *args:
    cargo run -p zendiloosd -- --config zendiloosd.dev.toml {{ args }}

# Print the OpenAPI document
[working-directory: 'backend']
openapi:
    @cargo run -q -p zendiloosd -- openapi

# Regenerate frontend/openapi.json and the TypeScript API types from the backend
api-types:
    cd backend && cargo run -q -p zendiloosd -- openapi > ../frontend/openapi.json
    cd frontend && npm run -s gen:api

# Fail if frontend/openapi.json or the TypeScript API types are stale
api-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    (cd backend && cargo run -q -p zendiloosd -- openapi) > "$tmp/openapi.json"
    diff -u frontend/openapi.json "$tmp/openapi.json"
    (cd frontend && npx openapi-typescript "$tmp/openapi.json" -o "$tmp/schema.d.ts" > /dev/null)
    diff -u frontend/src/shared/api/schema.d.ts "$tmp/schema.d.ts"

# Install the frontend dependencies exactly as locked
[working-directory: 'frontend']
web-install:
    npm ci

# Frontend dev server on http://localhost:5173; proxies /api to `just dev`
[working-directory: 'frontend']
web-dev:
    npm run dev

# Format the frontend code
[working-directory: 'frontend']
web-fmt:
    npm run -s format

# Frontend checks: formatting, lint, tests, types and production build
[working-directory: 'frontend']
web-check:
    npm run -s format:check
    npm run -s lint
    npm run -s test
    npm run -s build

# Build the Debian package in a debian:trixie container; the result lands in dist/
deb:
    docker build -t zendiloos-build -f packaging/build.Dockerfile .
    mkdir -p dist
    docker run --rm \
        -v "$PWD":/src:ro -v "$PWD/dist":/out \
        -v zendiloos-build-cargo:/opt/cargo/registry \
        -v zendiloos-build-target:/build/zendiloos/backend/target \
        -v zendiloos-build-npm:/root/.npm \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        zendiloos-build /src/packaging/build-deb.sh

# Licenses, security advisories and sources of the Rust dependencies
deny:
    cd backend && cargo deny check

# Debian 13 test VM: create, start, install, ssh, stop, destroy (see scripts/vm.sh)
[positional-arguments]
vm *args:
    scripts/vm.sh "$@"

# Creates, degrades, repairs and destroys a ZFS pool in the test VM through
# the API (the package must be installed and set up; see the script)
vm-storage-test:
    scripts/vm-storage-test.sh

# Shares over SMB and NFS with their permissions, in the test VM
vm-sharing-test:
    scripts/vm-sharing-test.sh
