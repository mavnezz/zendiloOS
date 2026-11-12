# Licences

<!-- Made by `just licences` from the software bill of materials; do not edit. -->

zendiloOS is under the Apache License 2.0. This page lists what its package
carries of others' work and under which licences. It is made from the
software bill of materials (`scripts/sbom.sh`), the same one every release
publishes beside its package, and `just check` holds both to the rules:
every licence is one `backend/deny.toml` allows, or an exception below for
one package of the web interface.

Allowed: 0BSD, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, BSL-1.0, ISC, MIT, Unicode-3.0, Unlicense, Zlib.

| Exception | Licence | Why |
| --- | --- | --- |
| @novnc/novnc | MPL-2.0 | noVNC, the console of virtual machines, bundled unmodified: the MPL binds its own files only, whose source is published (debian/copyright names it) |
| @fontsource-variable/geist | OFL-1.1 | the interface's font: the OFL lets fonts go with software, its text goes along |
| @fontsource-variable/jetbrains-mono | OFL-1.1 | the interface's monospaced font, likewise |

## The programs

zendiloosd and zendiloos-agent, built for x86-64 Linux, hold 260 crates of
others (build scripts and what only they use aside), and 24 of zendiloOS's
own (Apache-2.0).

| Licence | Crates | Which |
| --- | --- | --- |
| (Apache-2.0 OR MIT) AND BSD-3-Clause | 1 | encoding_rs 0.8.44 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 3 | unicode-ident 1.0.27, unicode-normalization 0.1.26, unicode-properties 0.1.5 |
| 0BSD | 1 | quoted_printable 0.5.2 |
| Apache-2.0 | 2 | listenfd 1.0.2, sync_wrapper 1.0.2 |
| Apache-2.0 AND ISC | 1 | ring 0.17.14 |
| Apache-2.0 OR BSL-1.0 | 1 | ryu 1.0.23 |
| Apache-2.0 OR BSL-1.0 OR MIT | 1 | whoami 2.1.3 |
| Apache-2.0 OR ISC OR MIT | 2 | rustls 0.23.46, rustls-native-certs 0.8.5 |
| Apache-2.0 OR MIT | 20 | atomic-waker 1.1.2, base64ct 1.8.3, cmov 0.5.4, const-oid 0.10.2, ctutils 0.4.8, equivalent 1.0.3, event-listener 5.4.2, fastrand 2.5.0, flume 0.12.0, idna_adapter 1.2.2, indexmap 2.14.2, multiversion_no_op 1.0.0, parking 2.2.1, phc 0.6.1, pin-project-lite 0.2.17, utf_types 1.0.0, utf8_iter 1.1.0, utf8parse 0.2.2, uuid 1.29.0, zeroize 1.9.1 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 2 | linux-raw-sys 0.12.1, rustix 1.1.6 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 1 | zerocopy 0.8.59 |
| BSD-3-Clause | 4 | subtle 2.6.1, zstd 0.13.4, zstd-safe 7.4.0, zstd-sys 2.1.2+zstd.1.5.7 |
| ISC | 2 | rustls-webpki 0.103.16, untrusted 0.9.0 |
| MIT | 44 | atoi 2.0.0, axum 0.8.9, axum-core 0.5.6, axum-extra 0.12.6, bytes 1.12.1, data-encoding 2.11.1, dotenvy 0.15.7, email_address 0.2.9, generic-array 0.14.9, http-body 1.1.0, http-body-util 0.1.5, hyper 1.12.0, hyper-util 0.1.21, lettre 0.11.23, libsqlite3-sys 0.37.0, matchers 0.2.0, minisign-verify 0.2.5, mio 1.2.4, nom 8.0.0, nu-ansi-term 0.50.3, sharded-slab 0.1.7, slab 0.4.12, spin 0.9.9, strsim 0.11.1, synstructure 0.14.0, sysinfo 0.40.1, tokio 1.54.1, tokio-macros 2.7.2, tokio-stream 0.1.19, tokio-tungstenite 0.29.0, tower 0.5.3, tower-http 0.7.1, tower-layer 0.3.3, tower-service 0.3.3, tracing 0.1.44, tracing-attributes 0.1.31, tracing-core 0.1.36, tracing-log 0.2.0, tracing-serde 0.2.0, tracing-subscriber 0.3.23, try-lock 0.2.5, want 0.3.2, winnow 1.0.5, zmij 1.0.23 |
| MIT AND BSD-3-Clause | 1 | matchit 0.8.4 |
| MIT OR Apache-2.0 | 149 | allocator-api2 0.2.21, annotate-snippets 0.12.17, anstream 1.0.0, anstyle 1.0.14, anstyle-parse 1.0.0, anstyle-query 1.1.5, anyhow 1.0.104, argon2 0.6.0, arraydeque 0.5.1, async-trait 0.1.92, base64 0.22.1, base64 0.23.1, bitflags 1.3.2, bitflags 2.13.2, blake2 0.11.0, block-buffer 0.10.4, block-buffer 0.12.1, cfg-if 1.0.5, chacha20 0.10.2, clap 4.6.7, clap_builder 4.6.7, clap_derive 4.6.7, clap_lex 1.1.1, colorchoice 1.0.5, cookie 0.18.2, core_detect 1.0.0, cpufeatures 0.2.17, cpufeatures 0.3.1, crc 3.4.0, crc-catalog 2.5.0, crossbeam-queue 0.3.14, crossbeam-utils 0.8.23, crypto-common 0.1.6, crypto-common 0.2.2, defmt 1.2.0, defmt-macros 1.2.0, defmt-parser 1.1.0, deranged 0.5.9, digest 0.10.7, digest 0.11.3, displaydoc 0.2.7, either 1.19.0, email-encoding 0.4.2, encoding_rs_io 0.1.8, errno 0.3.14, filetime 0.2.29, form_urlencoded 1.2.2, futures-channel 0.3.34, futures-core 0.3.34, futures-executor 0.3.34, futures-intrusive 0.5.0, futures-io 0.3.34, futures-macro 0.3.34, futures-sink 0.3.34, futures-task 0.3.34, futures-util 0.3.34, getrandom 0.2.17, getrandom 0.3.4, getrandom 0.4.3, granit-parser 2.1.0, hashbrown 0.16.1, hashbrown 0.17.1, hashlink 0.11.1, heck 0.5.0, hex 0.4.3, hkdf 0.13.0, hmac 0.13.0, http 1.5.0, httparse 1.10.1, httpdate 1.0.3, hybrid-array 0.4.15, idna 1.1.0, is_terminal_polyfill 1.70.2, itoa 1.0.18, lazy_static 1.5.1, libc 0.2.191, lock_api 0.4.14, log 0.4.35, md-5 0.11.0, mime 0.3.17, num-conv 0.2.2, num-traits 0.2.19, once_cell 1.21.4, openssl-probe 0.2.2, parking_lot 0.12.5, parking_lot_core 0.9.12, password-hash 0.6.1, pastey 0.2.3, percent-encoding 2.3.2, powerfmt 0.2.1, ppv-lite86 0.2.21, proc-macro2 1.0.107, quote 1.0.47, rand 0.10.3, rand 0.9.5, rand_chacha 0.9.0, rand_core 0.10.1, rand_core 0.9.5, regex-automata 0.4.18, regex-syntax 0.8.11, rustls-pki-types 1.15.1, scopeguard 1.2.0, serde 1.0.229, serde_core 1.0.229, serde_derive 1.0.229, serde_json 1.0.154, serde_path_to_error 0.1.20, serde_spanned 1.1.2, serde_urlencoded 0.7.1, serde-saphyr 2.1.1, sha1 0.10.7, sha1 0.11.0, sha2 0.10.9, sha2 0.11.1, signal-hook-registry 1.4.8, simdutf8 0.1.5, smallvec 1.16.4, socket2 0.6.5, sqlx 0.9.0, sqlx-core 0.9.0, sqlx-macros 0.9.0, sqlx-macros-core 0.9.0, sqlx-mysql 0.9.0, sqlx-postgres 0.9.0, sqlx-sqlite 0.9.0, stable_deref_trait 1.2.1, stringprep 0.1.5, syn 2.0.119, syn 3.0.9, tar 0.4.46, thiserror 2.0.21, thiserror-impl 2.0.21, thread_local 1.1.10, time 0.3.55, time-core 0.1.9, time-macros 0.2.32, tokio-rustls 0.26.6, toml 1.2.0+spec-1.1.0, toml_datetime 1.1.2+spec-1.1.0, toml_parser 1.1.5+spec-1.1.0, toml_writer 1.2.0+spec-1.1.0, tungstenite 0.29.0, typenum 1.20.1, unicode-bidi 0.3.18, unicode-width 0.2.3, url 2.5.8, utoipa 6.0.0, utoipa-axum 0.3.0, utoipa-gen 6.0.1 |
| Unicode-3.0 | 18 | icu_collections 2.4.0, icu_locale_core 2.4.0, icu_normalizer 2.4.0, icu_normalizer_data 2.4.0, icu_properties 2.4.0, icu_properties_data 2.4.0, icu_provider 2.4.0, litemap 0.8.3, potential_utf 0.1.6, tinystr 0.8.5, writeable 0.6.4, yoke 0.8.4, yoke-derive 0.8.4, zerofrom 0.1.8, zerofrom-derive 0.1.8, zerotrie 0.2.5, zerovec 0.11.9, zerovec-derive 0.11.7 |
| Unlicense OR MIT | 5 | aho-corasick 1.1.5, byteorder 1.5.0, jiff 0.2.39, jiff-core 0.1.2, memchr 2.8.3 |
| Zlib | 1 | foldhash 0.2.0 |
| Zlib OR Apache-2.0 OR MIT | 1 | tinyvec 1.13.3 |

## The web interface

Its bundle holds 54 packages from npm, as the build found them in it
(`frontend/dist/licences.json`; the tools that only build it are not
there). Their licence texts go along with it: on the NAS in
`/usr/share/zendiloos/webui/third-party-licences.txt`.

| Licence | Packages | Which |
| --- | --- | --- |
| Apache-2.0 | 3 | @internationalized/date 3.12.4, @internationalized/number 3.6.9, class-variance-authority 0.7.1 |
| BSD-2-Clause | 1 | entities 8.1.0 |
| ISC | 1 | lucide-vue-next 1.0.0 |
| MIT | 46 | @floating-ui/core 1.8.0, @floating-ui/dom 1.8.0, @floating-ui/utils 0.2.12, @floating-ui/vue 2.0.1, @intlify/core-base 11.4.18, @intlify/message-compiler 11.4.18, @intlify/shared 11.4.18, @tanstack/match-sorter-utils 9.2.6, @tanstack/query-core 5.106.1, @tanstack/virtual-core 3.19.11, @tanstack/vue-query 5.106.1, @tanstack/vue-virtual 3.13.52, @vue/devtools-api 6.6.4, @vue/devtools-api 8.2.1, @vue/devtools-kit 8.2.1, @vue/devtools-shared 8.2.1, @vue/reactivity 3.5.45, @vue/runtime-core 3.5.45, @vue/runtime-dom 3.5.45, @vue/shared 3.5.45, @vueuse/core 14.4.0, @vueuse/shared 14.4.0, @xterm/addon-fit 0.10.0, @xterm/xterm 5.5.0, aria-hidden 1.2.6, birpc 2.9.0, clsx 2.1.1, defu 6.1.7, hookable 5.5.3, linkify-it 6.1.0, markdown-it 15.0.3, mdurl 2.1.0, nostics 1.3.0, ohash 2.0.12, openapi-fetch 0.17.0, perfect-debounce 2.1.0, pinia 4.1.0, punycode.js 2.3.1, reka-ui 2.11.0, tailwind-merge 3.7.0, uc.micro 3.0.0, uqr 0.1.3, vue 3.5.45, vue-demi 0.14.10, vue-i18n 11.4.18, vue-router 5.4.0 |
| MPL-2.0 | 1 | @novnc/novnc 1.7.0 |
| OFL-1.1 | 2 | @fontsource-variable/geist 5.3.0, @fontsource-variable/jetbrains-mono 5.3.0 |

## Debian's packages

The package asks these of Debian 13, which installs them from its mirror
with their licences (`/usr/share/doc/<package>/copyright` on the NAS);
besides, the libraries the programs link (`${shlibs:Depends}`).

| Relation | Packages |
| --- | --- |
| Depends | acl, ca-certificates, cron | cron-daemon, gdisk, nftables, nginx, openssl, passwd, samba, smartmontools, systemd, util-linux, zfs-dkms, zfsutils-linux |
| Recommends | avahi-daemon, nfs-kernel-server, wsdd2 |
| Suggests | docker-compose, docker.io, libvirt-clients, libvirt-daemon-driver-qemu, ovmf, passt, qemu-system-x86, qemu-utils |
