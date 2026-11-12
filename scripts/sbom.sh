#!/usr/bin/env bash
# zendiloOS's software bill of materials (SBOM), CycloneDX 1.5 in JSON
# (docs/releases.md): the Rust crates its two programs are built from
# (cargo-cyclonedx, pinned by scripts/install-tools.sh), the npm packages the
# web interface's bundle holds (frontend/dist/licences.json, which the build
# writes) and, for a release, the package itself with the Debian packages it
# depends on, recommends and suggests. Debian's packages are named by what
# the package asks for: their versions are those of Debian 13's mirror at
# installation.
#
# Usage: scripts/sbom.sh sources OUT
#          what the sources bring, after the frontend's build
#        scripts/sbom.sh package DEB SOURCES OUT
#          SOURCES with the package DEB and the Debian packages it names
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
# Debian 13's, which the package is built for.
distro=debian-13

fail() { echo "sbom: $*" >&2; exit 1; }

work=$(mktemp -d)
cleanup() {
    rm -rf "$work"
    find "$root/backend/crates" -maxdepth 2 -name '*.cdx.json' -delete
}
trap cleanup EXIT

sources() {
    local out=$1 version time
    [[ -f $root/frontend/dist/licences.json ]] || fail "frontend/dist/licences.json is missing: build the frontend first"
    command -v cargo-cyclonedx >/dev/null || fail "cargo-cyclonedx is missing: scripts/install-tools.sh cargo-cyclonedx"
    version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/backend/Cargo.toml")
    # The commit's time, so that the same sources give the same SBOM.
    time=$(git -C "$root" log -1 --format=%ct 2>/dev/null || echo 0)
    # It writes one beside each crate of the workspace: the programs' are
    # taken, the others go. Build scripts and their dependencies run on the
    # machine that builds; they are not in what is shipped.
    (cd "$root/backend" && SOURCE_DATE_EPOCH=$time cargo cyclonedx --manifest-path crates/zendiloosd/Cargo.toml \
        --format json --spec-version 1.5 --target x86_64-unknown-linux-gnu --no-build-deps --quiet)
    for crate in zendiloosd zendiloos-agent; do
        mv "$root/backend/crates/$crate/$crate.cdx.json" "$work/$crate.json"
    done
    jq -n --slurpfile daemon "$work/zendiloosd.json" --slurpfile agent "$work/zendiloos-agent.json" \
        --slurpfile web "$root/frontend/dist/licences.json" --arg version "$version" --arg root "$root" \
        --argjson time "$time" '
        def npm: {
            type: "library",
            "bom-ref": "pkg:npm/\(.name | sub("^@"; "%40"))@\(.version)",
            name, version,
            purl: "pkg:npm/\(.name | sub("^@"; "%40"))@\(.version)",
            licenses: [{expression: .license}],
            externalReferences: (if .repository then [{type: "vcs", url: .repository}] else [] end),
            properties: [{name: "zendiloos:part", value: "web interface"}]
        };
        def rust: .properties = ((.properties // []) + [{name: "zendiloos:part", value: "programs"}]);
        ($daemon[0]) as $d | ($agent[0]) as $a | ($web[0] | map(npm)) as $npm
        | {
            bomFormat: "CycloneDX",
            specVersion: "1.5",
            version: 1,
            metadata: {
                timestamp: ($time | todate),
                tools: ($d.metadata.tools + [{vendor: "zendiloOS", name: "scripts/sbom.sh"}]),
                component: {
                    type: "application",
                    "bom-ref": "zendiloos",
                    name: "zendiloos",
                    version: $version,
                    licenses: [{expression: "Apache-2.0"}],
                    externalReferences: [{type: "vcs", url: "https://github.com/mavnezz/zendiloOS"}]
                }
            },
            components: (([$d.metadata.component, $a.metadata.component] | map(rust))
                + ($d.components + $a.components | unique_by(."bom-ref") | map(rust)) + $npm),
            dependencies: (
                [{ref: "zendiloos", dependsOn: ([$d.metadata.component, $a.metadata.component] + $npm | map(."bom-ref"))}]
                + ($d.dependencies + $a.dependencies | unique_by(.ref))
                + ($npm | map({ref: ."bom-ref", dependsOn: []})))
        }
        # Paths of the machine that made it say nothing of what it holds.
        | walk(if type == "string" then split($root) | join("zendiloOS") else . end)' >"$out"
}

package() {
    local deb=$1 sources=$2 out=$3 version arch sum
    [[ -f $deb ]] || fail "$deb is no package"
    version=$(dpkg-deb --field "$deb" Version)
    arch=$(dpkg-deb --field "$deb" Architecture)
    sum=$(sha256sum "$deb" | cut -d' ' -f1)
    {
        for field in Depends Recommends Suggests; do
            printf '%s\t%s\n' "$field" "$(dpkg-deb --field "$deb" "$field")"
        done
    } | jq -R -s --slurpfile sources "$sources" --arg version "$version" --arg arch "$arch" --arg sum "$sum" \
        --arg distro "$distro" --arg file "$(basename "$deb")" '
        # "cron | cron-daemon, libc6 (>= 2.34)": each package with its
        # constraint, the alternatives of one need noted.
        [split("\n")[] | select(length > 0) | split("\t") | .[0] as $field | .[1] | split(",")[]
            | gsub("^\\s+|\\s+$"; "") | select(length > 0) | . as $need | split("|")[]
            | gsub("^\\s+|\\s+$"; "") | capture("^(?<name>[^ (]+)(?:\\s*\\((?<constraint>[^)]*)\\))?")
            | {field: ($field | ascii_downcase), name, constraint, need: $need}] as $wanted
        | ($wanted | map({
            type: "application",
            "bom-ref": "pkg:deb/debian/\(.name)?arch=\($arch)&distro=\($distro)",
            name,
            purl: "pkg:deb/debian/\(.name)?arch=\($arch)&distro=\($distro)",
            properties: ([
                {name: "zendiloos:part", value: "Debian"},
                {name: "zendiloos:relation", value: .field},
                {name: "zendiloos:need", value: .need}
            ] + (if .constraint then [{name: "zendiloos:version-constraint", value: .constraint}] else [] end))
        }) | unique_by(."bom-ref")) as $debian
        | $sources[0]
        | .metadata.component += {
            version: $version,
            purl: "pkg:deb/zendiloos/zendiloos@\($version)?arch=\($arch)",
            hashes: [{alg: "SHA-256", content: $sum}],
            properties: [{name: "zendiloos:file", value: $file}]
        }
        | .components += $debian
        | .dependencies[0].dependsOn += ($debian | map(."bom-ref"))' >"$out"
}

case ${1:-} in
    sources) [[ $# == 2 ]] || fail "usage: $0 sources OUT"; sources "$2" ;;
    package) [[ $# == 4 ]] || fail "usage: $0 package DEB SOURCES OUT"; package "$2" "$3" "$4" ;;
    *) fail "usage: $0 sources OUT | package DEB SOURCES OUT" ;;
esac
