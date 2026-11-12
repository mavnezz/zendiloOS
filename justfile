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

# Everything CI checks; the tests run measured, against the coverage thresholds
check: version-check release-notes-test iso-network-test lint deny coverage-backend sqlx-check api-check web-check licences-check

# The crates carry the version of the newest entry in debian/changelog
# (Debian's ~ for a test release is Cargo's -)
version-check:
    #!/usr/bin/env bash
    set -euo pipefail
    changelog=$(sed -n '1s/^zendiloos (\([^)]*\)).*/\1/p' debian/changelog | tr '~' '-')
    crates=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' backend/Cargo.toml)
    if [[ $crates != "$changelog" ]]; then
        echo "backend/Cargo.toml says $crates, debian/changelog $changelog: set both in the release's pull request" >&2
        exit 1
    fi

# The notes a package carries, from the changelog or the commits since the
# last release tag (scripts/release-notes.sh), checked in a repository of
# their own
release-notes-test:
    scripts/release-notes-test.sh

# Test coverage of backend and frontend, against the thresholds CI holds
coverage: coverage-backend coverage-web

# The backend's tests, measured; thresholds in scripts/coverage-check.sh.
# The binaries' main.rs only wire the parts together (configuration,
# sockets, signals); the VM tests run them, unit tests cannot.
[working-directory: 'backend']
coverage-backend:
    mkdir -p ../dev/coverage
    cargo llvm-cov --workspace --locked --json --summary-only \
        --ignore-filename-regex '(zendiloos-agent|zendiloosd)/src/main\.rs' \
        --output-path ../dev/coverage/backend.json
    ../scripts/coverage-check.sh ../dev/coverage/backend.json

# The frontend's tests, measured; thresholds in frontend/vite.config.ts
[working-directory: 'frontend']
coverage-web:
    npm run -s test:coverage

# A browsable report of the backend's coverage: dev/coverage/backend/html/index.html
[working-directory: 'backend']
coverage-html:
    cargo llvm-cov --workspace --locked --html --output-dir ../dev/coverage/backend

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

# Frontend checks: formatting, lint, tests with their coverage, types and production build
[working-directory: 'frontend']
web-check:
    npm run -s format:check
    npm run -s lint
    npm run -s test:coverage
    npm run -s build

# The software bill of materials of what the sources bring, the crates and
# the web interface's bundle (scripts/sbom.sh), after the frontend's
# production build: dev/sbom/zendiloos.cdx.json. A release's adds its
# package and Debian's (CI, docs/releases.md).
sbom:
    mkdir -p dev/sbom
    scripts/sbom.sh sources dev/sbom/zendiloos.cdx.json

# Write docs/licences.md anew from the SBOM, after a production build
licences:
    cd frontend && npm run -s build
    just sbom
    node scripts/licences.mjs dev/sbom/zendiloos.cdx.json > docs/licences.md

# Fail if a licence is not one backend/deny.toml allows (or an exception in
# scripts/licences.mjs), or docs/licences.md is stale; after web-check's build
licences-check: sbom
    node scripts/licences.mjs --check dev/sbom/zendiloos.cdx.json

# Build the Debian package in a debian:trixie container; the result lands in dist/
deb:
    docker build -t zendiloos-build -f packaging/build.Dockerfile .
    mkdir -p dist
    docker run --rm \
        -v "$PWD":/src:ro -v "$PWD/dist":/out \
        -v zendiloos-build-cargo:/opt/cargo/registry \
        -v zendiloos-build-target:/build/zendiloos/backend/target \
        -v zendiloos-build-npm:/root/.npm \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e ZENDILOOS_VERSION="${ZENDILOOS_VERSION:-}" \
        zendiloos-build /src/packaging/build-deb.sh

# The installation ISO for the newest package in dist/: Debian 13's netinst
# image with an entry "Install zendiloOS" (docs/releases.md)
iso:
    scripts/build-iso.sh "$(ls -t dist/zendiloos_*_amd64.deb | head -1)" dist

# Installs from the newest ISO in dist/ into a fresh QEMU machine, unattended
iso-test:
    scripts/iso-test.sh

# The ISO's switch from ifupdown to systemd-networkd (packaging/iso/network.sh)
# against what Debian's installer writes, in a directory of its own
iso-network-test:
    scripts/iso-network-test.sh

# Makes the key pair releases are signed with (docs/releases.md): the public
# key goes into the repository, the secret one into a GitHub secret
release-key secret="~/.minisign/zendiloos-release.key":
    minisign -G -p packaging/release-signing.pub -s {{ secret }}

# Signs the package in dist/: SHA256SUMS and SHA256SUMS.minisig beside it
sign-release secret="~/.minisign/zendiloos-release.key":
    cd dist && sha256sum zendiloos_*.deb > SHA256SUMS
    minisign -S -s {{ secret }} -m dist/SHA256SUMS -t "zendiloos $(ls dist/zendiloos_*.deb | sed 's/.*zendiloos_\(.*\)_.*/\1/')"

# Package of an earlier commit or tag into dist/<ref>/, e.g. for the upgrade test
deb-at ref:
    #!/usr/bin/env bash
    set -euo pipefail
    tree=$(mktemp -d)
    trap 'git worktree remove --force "$tree"' EXIT
    git worktree add --detach "$tree" "{{ ref }}"
    mkdir -p "dist/{{ ref }}"
    docker build -t zendiloos-build -f packaging/build.Dockerfile .
    docker run --rm \
        -v "$tree":/src:ro -v "$PWD/dist/{{ ref }}":/out \
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

# Accounts, groups and personal folders in the VM: password rules, denied groups, homes
vm-accounts-test:
    scripts/vm-accounts-test.sh

# Share-level access in the VM: SMB and NFS by host, group members and disabled accounts without walks, apps in zendiloos-files, the migration from per-file ACLs
vm-permissions-test:
    scripts/vm-permissions-test.sh

# File services in the VM: SMB options, Time Machine, discovery, encryption
vm-services-test:
    scripts/vm-services-test.sh

# SSH as a service in the second VM: port, rules, off with open sessions, timer
vm-ssh-test:
    scripts/vm-ssh-test.sh

# Renaming the second VM: host name, /etc/hosts, Samba, Avahi, the certificate; put back at the end
vm-hostname-test:
    scripts/vm-hostname-test.sh

# The configuration backup: made on the second VM, restored on a fresh third one instead of its setup
vm-config-backup-test:
    scripts/vm-config-backup-test.sh

# The web interface's defences in the second VM: TLS, blocked addresses, account lock, sessions
vm-security-test:
    scripts/vm-security-test.sh

# Two-factor sign-in in the second VM: setup, codes, recovery codes, other address, the codes' limit and clearing it, reset
vm-two-factor-test:
    scripts/vm-two-factor-test.sh

# The Docker app in the second VM: projects with consent, networks, updates, moving Docker's data
vm-docker-test:
    scripts/vm-docker-test.sh

# Docker's volumes in the second VM: listed, backed up into a share, restored, kept by number
vm-docker-volumes-test:
    scripts/vm-docker-volumes-test.sh

# The App Center in the second VM: an app of the catalog as a project of one's own
vm-app-center-test:
    scripts/vm-app-center-test.sh

# Removable drives in the second VM: mounted when seen, ejected, pulled, guarded
vm-removable-test:
    scripts/vm-removable-test.sh

# Where each disk sits, in the second VM: a layout by port and serial, a disk blinking, the inventory
vm-enclosure-test:
    scripts/vm-enclosure-test.sh

# Administrators' projects in the VM: what runs is what was confirmed, binds pinned
vm-project-pins-test:
    scripts/vm-project-pins-test.sh

# Deleting pools in the VM: refused while something lives there, nothing changed while a mount is busy
vm-pool-delete-test:
    scripts/vm-pool-delete-test.sh

# Notification channels (a webhook in the VM, an email channel without server)
vm-notify-test:
    scripts/vm-notify-test.sh

# The firewall in the second VM: rules for SMB and a project, open connections while a change waits, new Docker bridges, the lock-out check, going back, a reboot
vm-firewall-test:
    scripts/vm-firewall-test.sh

# The web interface's settings and certificates in the second VM: saved settings after a restart, confirmations at once, chains acme.sh hands over, renewals nginx does not take
vm-web-server-test:
    scripts/vm-web-server-test.sh

# Virtual machines in the second VM (nested KVM): the plugin installed and set up, a VM booted into its firmware with its console's VNC greeting, an ISO added to the library from a share and by an upload, a VM of 2 disks and 2 cards changed while off and refused while it runs, its ISO ejected and inserted live, its load on the live WebSocket, the image kept while in use, a second storage location with a disk on each and its removal refused while in use, a card on an internal network the NAS has no address on and one directly on the VM's card, the VMs' log, no network of libvirt's
vm-vms-test:
    scripts/vm-vms-test.sh

# Backup tasks in the second VM: the plugin installed, a share on apps backed up to tank as a mirror and as versions (hard links, retention), to a USB-like drive when it is plugged in, to a test rsync daemon and over SSH on the VM itself (password never in ps, host key pinned), pulled back, a version restored and compared, links out of the source never followed
vm-backup-test:
    scripts/vm-backup-test.sh

# Network changes with confirmation, in the second VM (ZENDILOOS_VM=2)
vm-network-test:
    scripts/vm-network-test.sh

# The network taken over from ifupdown, kept once confirmed and given back when
# not, in a throwaway VM (ZENDILOOS_VM=3) that the script makes an ifupdown machine
vm-network-takeover-test:
    scripts/vm-network-takeover-test.sh

# Bridges, bonds and VLANs in a throwaway VM (ZENDILOOS_VM=3) started with two spare cards (ZENDILOOS_VM_EXTRA_NICS=2): kept once confirmed, gone back with their links when not, frames bridged past Docker's FORWARD chain, the firewall intact
vm-network-bridge-test:
    scripts/vm-network-bridge-test.sh

# An older package set up and in use, then the newest over it (see the script)
[positional-arguments]
vm-upgrade-test *args:
    scripts/vm-upgrade-test.sh "$@"
