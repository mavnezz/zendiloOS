#!/usr/bin/env bash
# Lists, for each DNS script of acme.sh as Debian ships it, the variables
# the script reads ($NAME, ${NAME…}, and acme.sh's saved values), one
# script a line: `dns_ionos IONOS_PREFIX IONOS_SECRET …`. The certificates
# context's tests check every provider it offers against this list
# (backend/crates/zendiloos-certificates/tests/acme-sh-dnsapi.txt), so a
# misspelt name or a script Debian does not have is caught without the
# package at hand.
#
#   scripts/acme-dnsapi-names.sh 3.1.1-1 dns_ionos dns_cf … > …/acme-sh-dnsapi.txt
set -euo pipefail
version=${1:?usage: acme-dnsapi-names.sh <Debian version> <script>…}
shift
for script in "$@"; do
    [[ $script =~ ^dns_[a-z0-9_]+$ ]] || { echo "not a script name: $script" >&2; exit 1; }
    body=$(curl -fsSL "https://sources.debian.org/data/main/a/acme.sh/$version/dnsapi/$script.sh")
    names=$(grep -oE '\$\{?[A-Za-z][A-Za-z0-9_]*|_(readaccountconf_mutable|readdomainconf|saveaccountconf_mutable|savedomainconf) [A-Za-z][A-Za-z0-9_]*' <<<"$body" \
        | sed -E 's/^\$\{?//; s/^_[a-z_]+ //' | sort -u | tr '\n' ' ')
    echo "$script ${names% }"
done
