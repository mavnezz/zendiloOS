#!/bin/sh
# The installer's last step (preseed.cfg's late_command, inside the new
# system): the network Debian's installer set up with ifupdown goes to
# systemd-networkd and systemd-resolved, which zendiloOS manages
# (docs/architecture.md, 6, "Network"):
#
#   - systemd-resolved is installed (systemd-networkd comes with systemd)
#   - each interface the installer set up in /etc/network/interfaces gets
#     /etc/systemd/network/05-zendiloos-<interface>.network, as zendiloOS
#     writes them: DHCP by the card's MAC address, or the static address
#     the installer was given; the other cards get none and stay down
#   - systemd-networkd and systemd-resolved are enabled; /etc/resolv.conf
#     leads to systemd-resolved's stub, which knows the name servers on the
#     first boot (from DHCP, or as given)
#   - ifupdown keeps the loopback only; what the installer wrote stays in
#     /etc/network/interfaces.zendiloos-previous
#
# Only what the installer writes is understood. Anything else (wireless, a
# fixed IPv6 address, a search domain, files in interfaces.d) leaves
# ifupdown as it is, and zendiloOS offers to take the network over later.
# It never fails the installation.
#
# Usage: network.sh [ROOT]
#   ROOT is / by default; scripts/iso-network-test.sh gives another, with
#   APT_GET and SYSTEMCTL set to stand-ins.
set -u

root=${1:-}
apt_get=${APT_GET:-apt-get}
systemctl=${SYSTEMCTL:-systemctl}
interfaces=$root/etc/network/interfaces
networkd=$root/etc/systemd/network

say() { echo "zendiloos network: $*"; }
keep() {
    say "$*; ifupdown keeps the network (zendiloOS offers to take it over)"
    exit 0
}

[ -f "$interfaces" ] || keep "there is no /etc/network/interfaces"
if [ -d "$root/etc/network/interfaces.d" ] && [ -n "$(ls -A "$root/etc/network/interfaces.d")" ]; then
    keep "interfaces.d is not empty"
fi
# Name servers in force, for a fixed address given none of its own.
servers=$(awk '$1 == "nameserver" && $2 != "127.0.0.53" { print $2 }' "$root/etc/resolv.conf" 2>/dev/null \
    | head -3 | tr '\n' ' ')

# One line per interface, `name|dhcp or static|address/prefix|gateway|name
# servers|ipv6`, or `refuse|why`. The installer's own forms only: comments,
# `source /etc/network/interfaces.d/*`, `auto` and `allow-hotplug`, the
# loopback, `inet dhcp`, `inet static` (address with its prefix or a
# netmask, gateway, dns-nameservers), `inet6 auto` and `inet6 dhcp`.
plan=$(awk -v servers="$servers" '
function refuse(why) { print "refuse|" why; refused = 1 }
function ipv4(text,   octets, count, i) {
    count = split(text, octets, ".")
    if (count != 4) return 0
    for (i = 1; i <= 4; i++) if (octets[i] !~ /^[0-9]+$/ || octets[i] + 0 > 255) return 0
    return 1
}
function prefix_of(mask,   octets, i, bits, value, seen_zero) {
    if (mask ~ /^[0-9]+$/) return (mask + 0 >= 1 && mask + 0 <= 32) ? mask + 0 : 0
    if (!ipv4(mask)) return 0
    split(mask, octets, ".")
    bits = 0
    for (i = 1; i <= 4; i++) {
        for (value = 128; value >= 1; value /= 2) {
            if (octets[i] + 0 >= value) {
                if (seen_zero) return 0
                bits++
                octets[i] -= value
            } else {
                seen_zero = 1
            }
        }
    }
    return bits
}
/^[ \t]*#/ || /^[ \t]*$/ { next }
/\\$/ { refuse("a continued line"); next }
$1 == "source" {
    current = ""
    if (NF != 2 || $2 != "/etc/network/interfaces.d/*") refuse("source " $2)
    next
}
$1 == "auto" || $1 == "allow-hotplug" {
    current = ""
    for (i = 2; i <= NF; i++) up[$i] = 1
    next
}
$1 == "iface" {
    current = "-"
    if (NF != 4) { refuse("iface " $2 " " $3 " " $4); next }
    name = $2; family = $3; method = $4
    if (name == "lo") {
        if (method != "loopback") refuse("lo: " family " " method)
        current = "lo"
        next
    }
    if (name !~ /^[A-Za-z0-9_-]+$/ || length(name) > 15) { refuse("the interface " name); next }
    if (!(name in seen)) { seen[name] = 1; names[++count] = name }
    if (family == "inet" && (method == "dhcp" || method == "static")) {
        if (name in mode) { refuse(name ": a second inet stanza"); next }
        mode[name] = method; current = name; inet6 = 0
        next
    }
    if (family == "inet6" && (method == "auto" || method == "dhcp")) {
        current = name; inet6 = 1
        next
    }
    refuse(name ": " family " " method)
    next
}
{
    if (current == "") { refuse($1 " stands outside a stanza"); next }
    if (current == "-") next
    if (current == "lo") { refuse("lo: " $1); next }
    value = $0
    sub(/^[ \t]*[^ \t]+[ \t]*/, "", value)
    sub(/[ \t]+$/, "", value)
    if ($1 == "dns-nameservers") { dns[current] = dns[current] " " value; next }
    if (inet6 || mode[current] != "static") { refuse(current ": " $1); next }
    if ($1 == "address") address[current] = value
    else if ($1 == "netmask") netmask[current] = value
    else if ($1 == "gateway") gateway[current] = value
    else refuse(current ": " $1)
}
END {
    for (i = 1; i <= count; i++) {
        name = names[i]
        if (!(name in up)) { refuse(name ": not brought up at boot"); continue }
        if (!(name in mode)) { refuse(name ": no IPv4"); continue }
        cidr = ""; via = ""
        if (mode[name] == "static") {
            split(address[name], parts, "/")
            prefix = (2 in parts) ? prefix_of(parts[2]) : prefix_of(netmask[name])
            if (!ipv4(parts[1]) || !prefix) { refuse(name ": the address " address[name] " " netmask[name]); continue }
            if ((name in gateway) && !ipv4(gateway[name])) { refuse(name ": the gateway " gateway[name]); continue }
            cidr = parts[1] "/" prefix
            via = gateway[name]
        }
        list = dns[name]
        if (list == "" && mode[name] == "static") list = servers
        split(list, each, " ")
        list = ""; taken = 0
        for (j = 1; (j in each) && taken < 3; j++) {
            if (each[j] !~ /^[0-9A-Fa-f.:]+$/) { refuse(name ": the name server " each[j]); break }
            list = list (taken++ ? " " : "") each[j]
        }
        print name "|" mode[name] "|" cidr "|" via "|" list
    }
    if (count == 0) refuse("no interface but the loopback")
}' "$interfaces")

case $plan in
    *refuse\|*) keep "$(printf '%s\n' "$plan" | sed -n 's/^refuse|//p' | paste -sd ';' -)" ;;
esac
[ -n "$plan" ] || keep "nothing to set up"
if [ -z "$root" ]; then
    for name in $(printf '%s\n' "$plan" | cut -d'|' -f1); do
        [ -d "/sys/class/net/$name/wireless" ] && keep "$name is a wireless card"
    done
fi

say "installing systemd-resolved"
DEBIAN_FRONTEND=noninteractive $apt_get install --yes systemd-resolved </dev/null \
    || keep "systemd-resolved could not be installed"

mkdir -p "$networkd"
printf '%s\n' "$plan" | while IFS='|' read -r name mode cidr via list; do
    file=$networkd/05-zendiloos-$name.network
    {
        echo "# Written by zendiloOS; changes here are replaced. Change the network in its interface."
        printf '[Match]\nName=%s\n\n[Network]\n' "$name"
        if [ "$mode" = dhcp ]; then echo "DHCP=ipv4"; else echo "DHCP=no"; fi
        echo "IPv6AcceptRA=yes"
        [ -n "$cidr" ] && echo "Address=$cidr"
        [ -n "$via" ] && echo "Gateway=$via"
        for server in $list; do echo "DNS=$server"; done
        if [ "$mode" = dhcp ]; then
            printf '\n[DHCPv4]\nClientIdentifier=mac\nUseDomains=yes\n'
            [ -n "$list" ] && echo "UseDNS=no"
        fi
        [ -n "$list" ] && printf '\n[IPv6AcceptRA]\nUseDNS=no\n'
    } >"$file"
    say "$name: $mode${cidr:+ $cidr}${via:+ via $via}${list:+, name servers $list}"
done

cp -p "$interfaces" "$interfaces.zendiloos-previous"
cat >"$interfaces" <<'EOF'
# zendiloOS manages the network with systemd-networkd
# (/etc/systemd/network/05-zendiloos-*.network): change it in its web
# interface. What ifupdown had here is in interfaces.zendiloos-previous.

auto lo
iface lo inet loopback
EOF
ln -sfn ../run/systemd/resolve/stub-resolv.conf "$root/etc/resolv.conf"
$systemctl enable systemd-networkd.service systemd-resolved.service || say "could not enable the services"
say "systemd-networkd and systemd-resolved manage the network from the first boot"
