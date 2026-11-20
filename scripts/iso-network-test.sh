#!/usr/bin/env bash
# Tests packaging/iso/network.sh, the installer's switch from ifupdown to
# systemd-networkd, against what Debian's installer writes, in a directory
# of its own: apt-get and systemctl are stand-ins that record their
# arguments. The files it writes must be the ones zendiloOS's agent writes
# for the same settings (backend/crates/zendiloos-agent/src/network.rs).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
script=$root/packaging/iso/network.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }

HEADER='# Written by zendiloOS; changes here are replaced. Change the network in its interface.'

# A new system with `interfaces` as the installer wrote it.
machine() {
    local name=$1 text=$2
    local dir=$work/$awk/$name
    mkdir -p "$dir/etc/network/interfaces.d" "$dir/bin"
    printf '%s' "$text" >"$dir/etc/network/interfaces"
    printf 'domain fritz.box\nsearch fritz.box\nnameserver 192.168.1.1\n' >"$dir/etc/resolv.conf"
    for tool in apt-get systemctl; do
        printf '#!/bin/sh\necho "%s $*" >>%s/calls\n' "$tool" "$dir" >"$dir/bin/$tool"
        chmod +x "$dir/bin/$tool"
    done
    echo "$dir"
}

run() {
    local dir=$1
    APT_GET=$dir/bin/apt-get SYSTEMCTL=$dir/bin/systemctl sh "$script" "$dir" >"$dir/out" 2>&1 \
        || fail "$(basename "$dir"): the script failed: $(cat "$dir/out")"
}

expect_file() {
    local dir=$1 file=$2 expected=$3
    [[ -f $dir/$file ]] || fail "$(basename "$dir"): no $file: $(cat "$dir/out")"
    diff -u <(printf '%s\n' "$expected") "$dir/$file" || fail "$(basename "$dir"): $file"
}

# Taken over: the loopback only, the installer's file kept, the stub, both
# services enabled.
expect_switched() {
    local dir=$1 before=$2
    grep -q '^iface lo inet loopback$' "$dir/etc/network/interfaces" || fail "$(basename "$dir"): the loopback"
    ! grep -q 'inet dhcp\|inet static\|source' "$dir/etc/network/interfaces" || fail "$(basename "$dir"): ifupdown kept more"
    diff -u <(printf '%s' "$before") "$dir/etc/network/interfaces.zendiloos-previous" || fail "$(basename "$dir"): the copy"
    [[ $(readlink "$dir/etc/resolv.conf") == ../run/systemd/resolve/stub-resolv.conf ]] || fail "$(basename "$dir"): resolv.conf"
    grep -qx 'apt-get install --yes systemd-resolved' "$dir/calls" || fail "$(basename "$dir"): systemd-resolved"
    grep -qx 'systemctl enable systemd-networkd.service systemd-resolved.service' "$dir/calls" \
        || fail "$(basename "$dir"): the services"
    [[ ! -e $dir/etc/resolv.conf.zendiloos-installer ]] || fail "$(basename "$dir"): the copy of resolv.conf left"
}

# Left to ifupdown: nothing changed, nothing installed.
expect_kept() {
    local dir=$1 before=$2 why=$3
    diff -u <(printf '%s' "$before") "$dir/etc/network/interfaces" || fail "$(basename "$dir"): changed"
    [[ ! -e $dir/calls && ! -d $dir/etc/systemd/network && -f $dir/etc/resolv.conf && ! -L $dir/etc/resolv.conf ]] \
        || fail "$(basename "$dir"): touched"
    grep -q "$why" "$dir/out" || fail "$(basename "$dir"): does not say why ($why): $(cat "$dir/out")"
}

# Gone back to ifupdown after a failure on the way: `interfaces` and
# `resolv.conf` as the installer left them, no file of zendiloOS's, nothing
# of the switch left behind.
expect_back() {
    local dir=$1 before=$2 why=$3
    diff -u <(printf '%s' "$before") "$dir/etc/network/interfaces" || fail "$(basename "$dir"): interfaces changed"
    [[ -f $dir/etc/resolv.conf && ! -L $dir/etc/resolv.conf ]] || fail "$(basename "$dir"): resolv.conf is not a file"
    grep -qx 'nameserver 192.168.1.1' "$dir/etc/resolv.conf" || fail "$(basename "$dir"): resolv.conf: $(cat "$dir/etc/resolv.conf")"
    [[ -z $(find "$dir/etc/systemd/network" -type f 2>/dev/null) ]] || fail "$(basename "$dir"): files of systemd-networkd left"
    [[ ! -e $dir/etc/network/interfaces.zendiloos-previous && ! -e $dir/etc/resolv.conf.zendiloos-installer ]] \
        || fail "$(basename "$dir"): copies left"
    grep -q "$why" "$dir/out" || fail "$(basename "$dir"): does not say why ($why): $(cat "$dir/out")"
    grep -q 'ifupdown keeps' "$dir/out" || fail "$(basename "$dir"): does not say ifupdown keeps it: $(cat "$dir/out")"
}

# Debian has mawk, not gawk: the script runs with each there is here.
cases() {
echo "DHCP, with IPv6 from the router ($awk)"
dhcp='# This file describes the network interfaces available on your system
# and how to activate them. For more information, see interfaces(5).

source /etc/network/interfaces.d/*

# The loopback network interface
auto lo
iface lo inet loopback

# The primary network interface
allow-hotplug enp2s0
iface enp2s0 inet dhcp
# This is an autoconfigured IPv6 interface
iface enp2s0 inet6 auto
'
dir=$(machine dhcp "$dhcp")
run "$dir"
expect_file "$dir" etc/systemd/network/05-zendiloos-enp2s0.network "$HEADER
[Match]
Name=enp2s0

[Network]
DHCP=ipv4
IPv6AcceptRA=yes

[DHCPv4]
ClientIdentifier=mac
UseDomains=yes"
expect_switched "$dir" "$dhcp"
[[ $(find "$dir/etc/systemd/network" -type f | wc -l) == 1 ]] || fail "dhcp: a file for a card the installer did not set up"
# Once more: nothing is left to switch.
run "$dir"
grep -q 'ifupdown keeps' "$dir/out" || fail "dhcp: a second run changed something"

echo "A fixed address, with a netmask, a gateway and name servers"
fixed='auto lo
iface lo inet loopback

allow-hotplug enp2s0
iface enp2s0 inet static
	address 192.168.1.10
	netmask 255.255.255.0
	gateway 192.168.1.1
	# dns-* options are implemented by the resolvconf package, if installed
	dns-nameservers 192.168.1.1 9.9.9.9
'
dir=$(machine fixed "$fixed")
run "$dir"
expect_file "$dir" etc/systemd/network/05-zendiloos-enp2s0.network "$HEADER
[Match]
Name=enp2s0

[Network]
DHCP=no
IPv6AcceptRA=yes
Address=192.168.1.10/24
Gateway=192.168.1.1
DNS=192.168.1.1
DNS=9.9.9.9

[IPv6AcceptRA]
UseDNS=no"
expect_switched "$dir" "$fixed"

echo "A fixed address in the installer's own form, without name servers: those in resolv.conf"
cidr='auto lo
iface lo inet loopback

allow-hotplug enp2s0
iface enp2s0 inet static
	address 10.0.0.5/8
'
dir=$(machine cidr "$cidr")
run "$dir"
expect_file "$dir" etc/systemd/network/05-zendiloos-enp2s0.network "$HEADER
[Match]
Name=enp2s0

[Network]
DHCP=no
IPv6AcceptRA=yes
Address=10.0.0.5/8
DNS=192.168.1.1

[IPv6AcceptRA]
UseDNS=no"

echo "What the installer may write but zendiloOS does not carry over stays with ifupdown"
wireless='auto lo
iface lo inet loopback

allow-hotplug wlp3s0
iface wlp3s0 inet dhcp
	wpa-ssid home
	wpa-psk secret
'
dir=$(machine wireless "$wireless")
run "$dir"
expect_kept "$dir" "$wireless" 'wlp3s0: wpa-ssid'
ipv6='auto lo
iface lo inet loopback

allow-hotplug enp2s0
iface enp2s0 inet dhcp
iface enp2s0 inet6 static
	address 2001:db8::5/64
'
dir=$(machine ipv6 "$ipv6")
run "$dir"
expect_kept "$dir" "$ipv6" 'enp2s0: inet6 static'
search='allow-hotplug enp2s0
iface enp2s0 inet static
	address 192.168.1.10/24
	dns-search example.org
'
dir=$(machine search "$search")
run "$dir"
expect_kept "$dir" "$search" 'enp2s0: dns-search'
dir=$(machine sourced "$dhcp")
echo 'iface enp3s0 inet dhcp' >"$dir/etc/network/interfaces.d/enp3s0"
run "$dir"
expect_kept "$dir" "$dhcp" 'interfaces.d is not empty'
dir=$(machine failing "$dhcp")
printf '#!/bin/sh\nexit 100\n' >"$dir/bin/apt-get"
run "$dir"
expect_back "$dir" "$dhcp" 'systemd-resolved could not be installed'

echo "Whatever fails on the way, ifupdown keeps the network as the installer left it"
# systemd-resolved's package points resolv.conf at its stub when it is
# configured; then the run fails (another package's postinst, a trigger).
dir=$(machine postinst "$dhcp")
printf '#!/bin/sh\nmv %s/etc/resolv.conf %s/etc/.resolv.conf.systemd-resolved.bak\nln -s ../run/systemd/resolve/stub-resolv.conf %s/etc/resolv.conf\nexit 100\n' \
    "$dir" "$dir" "$dir" >"$dir/bin/apt-get"
run "$dir"
expect_back "$dir" "$dhcp" 'systemd-resolved could not be installed'
dir=$(machine enabling "$dhcp")
printf '#!/bin/sh\necho "systemctl $*" >>%s/calls\n[ "$1" = enable ] && exit 1\nexit 0\n' "$dir" >"$dir/bin/systemctl"
run "$dir"
expect_back "$dir" "$dhcp" 'could not be enabled'
grep -qx 'systemctl disable systemd-networkd.service systemd-resolved.service' "$dir/calls" \
    || fail "enabling: not disabled again: $(cat "$dir/calls")"
dir=$(machine unwritable "$dhcp")
mkdir -p "$dir/etc/systemd/network/05-zendiloos-enp2s0.network"
run "$dir"
expect_back "$dir" "$dhcp" '05-zendiloos-enp2s0.network cannot be written'
grep -qx 'systemctl disable systemd-networkd.service systemd-resolved.service' "$dir/calls" \
    || fail "unwritable: not disabled again: $(cat "$dir/calls")"
}

tested=0
for awk in mawk gawk; do
    command -v "$awk" >/dev/null || continue
    mkdir -p "$work/$awk/bin"
    ln -s "$(command -v "$awk")" "$work/$awk/bin/awk"
    PATH=$work/$awk/bin:$PATH cases
    tested=$((tested + 1))
done
(( tested > 0 )) || { awk=awk; cases; }

echo "Passed: the installer's network goes to systemd-networkd, the rest stays with ifupdown"
