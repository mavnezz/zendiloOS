#!/usr/bin/env bash
# Installs the development tools in the versions CI uses, on x86-64 Linux:
# just, cargo-deny, cargo-llvm-cov and cargo-cyclonedx (the SBOM,
# scripts/sbom.sh) as their projects' prebuilt binaries, each checked
# against its SHA-256, and sqlx-cli (SQLite only), which has no prebuilt
# binaries and is built with cargo, unless that version is there already.
# Rust itself comes from rustup and rust-toolchain.toml.
#
# Usage: scripts/install-tools.sh [TOOL ...]
#   TOOL       just, cargo-deny, cargo-llvm-cov, cargo-cyclonedx, sqlx-cli
#              (default: all)
#   TOOLS_DIR  where the programs go (default: ~/.local/bin)
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dest=${TOOLS_DIR:-$HOME/.local/bin}

just_version=1.45.0
just_sha256=dc3f958aaf8c6506dd90426e9b03f86dd15e74a6467ee0e54929f750af3d9e49
deny_version=0.20.2
deny_sha256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
llvm_cov_version=0.9.1
llvm_cov_sha256=3fca950394a3c49457657c158b1619cec8dfd2647ae5b48746734c0ab969a522
cyclonedx_version=0.5.9
cyclonedx_sha256=9bd3e599314f50810c9d98b8b68a617ff9d3cc20873968d90b29d121f6b226ff
# The version of sqlx in backend/Cargo.toml.
sqlx_version=0.9.0

say() { printf '\033[1m%s\033[0m\n' "$*"; }

[[ $(uname -sm) == "Linux x86_64" ]] || {
    echo "only x86-64 Linux has prebuilt tools here; install them with cargo install" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$dest"

# fetch URL SHA256 PATH-IN-ARCHIVE NAME
fetch() {
    local url=$1 sha256=$2 member=$3 name=$4
    curl -sfL --retry 3 -o "$work/archive" "$url"
    echo "$sha256  $work/archive" | sha256sum --check --status \
        || { echo "$url does not match its checksum" >&2; exit 1; }
    # gzip or xz, as tar tells by itself.
    tar -xf "$work/archive" -C "$work" "$member"
    install -m 0755 "$work/$member" "$dest/$name"
    rm -rf "${work:?}"/*
}

install_tool() {
    case $1 in
        just)
            say "just $just_version"
            fetch "https://github.com/casey/just/releases/download/$just_version/just-$just_version-x86_64-unknown-linux-musl.tar.gz" \
                "$just_sha256" just just
            ;;
        cargo-deny)
            say "cargo-deny $deny_version"
            fetch "https://github.com/EmbarkStudios/cargo-deny/releases/download/$deny_version/cargo-deny-$deny_version-x86_64-unknown-linux-musl.tar.gz" \
                "$deny_sha256" "cargo-deny-$deny_version-x86_64-unknown-linux-musl/cargo-deny" cargo-deny
            ;;
        cargo-llvm-cov)
            say "cargo-llvm-cov $llvm_cov_version"
            fetch "https://github.com/taiki-e/cargo-llvm-cov/releases/download/v$llvm_cov_version/cargo-llvm-cov-x86_64-unknown-linux-musl.tar.gz" \
                "$llvm_cov_sha256" cargo-llvm-cov cargo-llvm-cov
            ;;
        cargo-cyclonedx)
            say "cargo-cyclonedx $cyclonedx_version"
            fetch "https://github.com/CycloneDX/cyclonedx-rust-cargo/releases/download/cargo-cyclonedx-$cyclonedx_version/cargo-cyclonedx-x86_64-unknown-linux-musl.tar.xz" \
                "$cyclonedx_sha256" cargo-cyclonedx-x86_64-unknown-linux-musl/cargo-cyclonedx cargo-cyclonedx
            ;;
        sqlx-cli)
            if [[ $("$dest/sqlx" --version 2>/dev/null) == "sqlx-cli $sqlx_version" ]]; then
                say "sqlx-cli $sqlx_version is there"
                return
            fi
            say "sqlx-cli $sqlx_version (built with cargo)"
            # In the repository, so that rust-toolchain.toml picks the compiler.
            (cd "$root" && cargo install sqlx-cli --version "$sqlx_version" --locked \
                --no-default-features --features sqlite --root "$work/sqlx")
            install -m 0755 "$work/sqlx/bin/sqlx" "$work/sqlx/bin/cargo-sqlx" "$dest/"
            rm -rf "$work/sqlx"
            ;;
        *)
            echo "unknown tool: $1 (just, cargo-deny, cargo-llvm-cov, cargo-cyclonedx, sqlx-cli)" >&2
            exit 2
            ;;
    esac
}

(($#)) || set -- just cargo-deny cargo-llvm-cov cargo-cyclonedx sqlx-cli
for tool in "$@"; do
    install_tool "$tool"
done
case :$PATH: in
    *":$dest:"*) ;;
    *) echo "Add $dest to PATH." ;;
esac
