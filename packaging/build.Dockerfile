# Build environment of the zendiloos Debian package (`just deb`). git reads
# the commits a development build's notes list (scripts/release-notes.sh).
# Pinned by digest, as tags can be moved. A newer one:
# `docker buildx imagetools inspect debian:trixie`, its index digest here.
FROM debian:trixie@sha256:9c6e381421d0f48d1b8133e573f33c185ee16cbf505eda9c4a8a4791b25ea8a6

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl debhelper git rsync rustup xz-utils \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/node/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

# Node.js 24: Debian 13 ships 20, the frontend tools need 22 or later.
# Pinned, and verified against the checksum the release publishes.
ARG NODE_VERSION=24.22.1
ARG NODE_SHA256=6c172a9ab6718f7b801fb5a7bd486af2ee5e32758086a04b84fcce2aa77095e6
RUN curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
 && echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - \
 && mkdir -p /opt/node \
 && tar -xJf /tmp/node.tar.xz -C /opt/node --strip-components=1 \
 && rm /tmp/node.tar.xz

# Rust exactly as pinned in rust-toolchain.toml.
COPY rust-toolchain.toml /opt/toolchain/
RUN cd /opt/toolchain && rustup show && cargo --version
