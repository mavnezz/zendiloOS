# Build environment of the zendiloos Debian package (`just deb`). git reads
# the commits a development build's notes list (scripts/release-notes.sh).
# Pinned by digest, as tags can be moved: the digest of the index, which
# names the image of each architecture, so the same line pins the image an
# amd64 machine runs and the one an arm64 machine runs. A newer one:
# `docker buildx imagetools inspect debian:trixie`, its index digest here.
FROM debian:trixie@sha256:9c6e381421d0f48d1b8133e573f33c185ee16cbf505eda9c4a8a4791b25ea8a6

# The package for the other architecture is cross-compiled
# (`dpkg-buildpackage --host-arch`): Debian's cross compiler for it, and its
# C library with its headers and libgcc as packages of that architecture
# (multiarch), which the compiler builds and links against and from which
# dpkg-shlibdeps tells the package's dependencies, as it does for the own
# architecture.
RUN set -eu; \
    case $(dpkg --print-architecture) in \
        amd64) other=arm64 cross=gcc-aarch64-linux-gnu ;; \
        arm64) other=amd64 cross=gcc-x86-64-linux-gnu ;; \
        *) echo "no zendiloOS for $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    dpkg --add-architecture "$other"; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl debhelper git rsync rustup xz-utils \
        "$cross" "libc6-dev:$other" "libgcc-s1:$other"; \
    rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/node/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# Node.js 24: Debian 13 ships 20, the frontend tools need 22 or later.
# Pinned, and verified against the checksum the release publishes for this
# machine's architecture. What it builds, the web interface, is the same
# for every architecture.
ARG NODE_VERSION=24.22.1
ARG NODE_SHA256_X64=6c172a9ab6718f7b801fb5a7bd486af2ee5e32758086a04b84fcce2aa77095e6
ARG NODE_SHA256_ARM64=dbabf7a1cbf947e6059ffad2c27b80041db97dea1e553b62afdee7f13fca336e
RUN set -eu; \
    case $(dpkg --print-architecture) in \
        amd64) node=x64 sum=$NODE_SHA256_X64 ;; \
        arm64) node=arm64 sum=$NODE_SHA256_ARM64 ;; \
    esac; \
    curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node}.tar.xz"; \
    echo "${sum}  /tmp/node.tar.xz" | sha256sum -c -; \
    mkdir -p /opt/node; \
    tar -xJf /tmp/node.tar.xz -C /opt/node --strip-components=1; \
    rm /tmp/node.tar.xz

# Rust exactly as pinned in rust-toolchain.toml, with the standard library
# of both architectures the package is built for.
COPY rust-toolchain.toml /opt/toolchain/
RUN cd /opt/toolchain && rustup show \
 && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
 && cargo --version
