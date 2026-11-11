#!/usr/bin/env bash
# Tests taking the network over from ifupdown in a throwaway test VM,
# through the HTTPS API:
#
#   0. the VM (Debian's cloud image, which runs systemd-networkd through
#      netplan) becomes a machine as Debian's installer leaves one: ifupdown
#      with DHCP for its card in /etc/network/interfaces, dhcpcd as its DHCP
#      client, systemd-networkd and systemd-resolved off and disabled,
#      /etc/resolv.conf a file, and the first time systemd-resolved removed
#   1. taken over and confirmed: systemd-resolved installed by the job,
#      zendiloOS's file for the card (DHCP by the MAC address, asking for the
#      address the card had), ifupdown left with the loopback, the
#      originals kept; the address is the same, systemd-networkd uses the
#      file, names resolve
#   2. ifupdown again, taken over and not confirmed: two minutes later
#      ifupdown has the network back by itself, exactly as it was
#   3. ifupdown again, taken over, and the agent restarted before it is
#      confirmed: the agent gives the network back as it starts
#
# The VM is reached through QEMU's user networking (SSH and HTTPS forwarded
# to its address, 10.0.2.15, which QEMU's DHCP hands out again), so every
# switch here keeps DHCP working; while the card changes hands it is gone
# for a few seconds, and the script waits for it.
#
# Run it against a VM that nothing else needs, ZENDILOOS_VM=3 by default:
#   ZENDILOOS_VM=3 just vm create && ZENDILOOS_VM=3 just vm start && ZENDILOOS_VM=3 just vm install
#   just vm-network-takeover-test
#   ZENDILOOS_VM=3 just vm destroy
# The package must be installed; an administrator is set up if there is
# none (ZENDILOOS_USER and ZENDILOOS_PASSWORD, defaults admin and "correct
# horse battery"). ZENDILOOS_TAKEOVER_KEEP_RESOLVED=1 leaves
# systemd-resolved installed in step 0, so the job does not install it.
# Takes about ten minutes; the VM is left with ifupdown.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-3}
[[ $ZENDILOOS_VM != 1 ]] || { echo "not on VM 1: it holds the LAN test (ZENDILOOS_VM=3)" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
check() { printf '   ✓ %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk --max-time 15 -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 test=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$test" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

sign_in() {
    curl -sk --max-time 15 -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/csrf"
}

# What ifupdown manages, written by the VM's root in a unit of its own: the
# card goes away when systemd-networkd stops, and so does this SSH session.
cat >"$work/to-ifupdown.sh" <<'SCRIPT'
#!/bin/bash
set -euxo pipefail
exec >/root/zendiloos-to-ifupdown.log 2>&1
rm -f /root/zendiloos-ifupdown.done
export DEBIAN_FRONTEND=noninteractive
card=$(ip -o -4 route show default | awk '{print $5; exit}')
[[ -n $card ]] || card=$(cat /root/zendiloos-card)
echo "$card" >/root/zendiloos-card
# Nothing of zendiloOS's or of an earlier run: a machine fresh from the installer.
systemctl stop systemd-networkd.socket systemd-networkd.service || true
rm -f /etc/systemd/network/05-zendiloos-* /etc/network/zendiloos-takeover.json \
    /etc/network/interfaces.zendiloos-previous /etc/resolv.conf.zendiloos-previous
mkdir -p /etc/network/interfaces.d /root/zendiloos-saved
find /etc/network/interfaces.d -mindepth 1 -maxdepth 1 -exec mv -t /root/zendiloos-saved {} +
printf 'auto lo\niface lo inet loopback\n' >/etc/network/interfaces
if ! command -v ifup >/dev/null || ! command -v dhcpcd >/dev/null; then
    # Fetched while systemd-networkd still had the card.
    systemctl start systemd-networkd.service
    for _ in $(seq 30); do ip -4 -o addr show dev "$card" | grep -q inet && break; sleep 1; done
    apt-get install -y -q ifupdown dhcpcd-base
fi
if [[ ${REMOVE_RESOLVED:-} == 1 ]] && dpkg -s systemd-resolved >/dev/null 2>&1; then
    apt-get purge -y -q systemd-resolved
fi
# Names resolve with ifupdown too: QEMU's server, and public ones beside it,
# which the VM's DNS settings name for systemd-resolved as well.
rm -f /etc/resolv.conf
printf 'nameserver 10.0.2.3\nnameserver 1.1.1.1\nnameserver 9.9.9.9\n' >/etc/resolv.conf
grep -q '^nohook resolv.conf' /etc/dhcpcd.conf 2>/dev/null || echo 'nohook resolv.conf' >>/etc/dhcpcd.conf
# netplan and cloud-init no longer set up the network.
find /etc/netplan -maxdepth 1 -name '*.yaml' -exec mv -t /root/zendiloos-saved {} + 2>/dev/null || true
rm -f /run/systemd/network/10-netplan-*
mkdir -p /etc/cloud/cloud.cfg.d
echo 'network: {config: disabled}' >/etc/cloud/cloud.cfg.d/99-zendiloos-ifupdown.cfg
systemctl disable --now systemd-networkd.socket systemd-networkd.service systemd-networkd-wait-online.service
if systemctl cat systemd-resolved.service >/dev/null 2>&1; then
    systemctl disable --now systemd-resolved.service
fi
# As Debian's installer writes it.
cat >/etc/network/interfaces <<EOF
# This file describes the network interfaces available on your system
# and how to activate them. For more information, see interfaces(5).

source /etc/network/interfaces.d/*

# The loopback network interface
auto lo
iface lo inet loopback

# The primary network interface
allow-hotplug $card
iface $card inet dhcp
EOF
systemctl enable networking.service
# Started again after a stop, networking.service brings up allow-hotplug
# cards too, as udev does at boot; its stop leaves this mark.
systemctl stop networking.service || true
mkdir -p /run/network && touch /run/network/restart-hotplug
systemctl start networking.service
for _ in $(seq 60); do ip -4 -o addr show dev "$card" | grep -q inet && break; sleep 1; done
ip -4 -o addr show dev "$card" | grep -q inet || systemctl start "ifup@$card.service"
ip -4 -o addr show dev "$card" | awk '{print $4}' | cut -d/ -f1 >/root/zendiloos-address
touch /root/zendiloos-ifupdown.done
SCRIPT

wait_for_ssh() {
    wait_for "SSH to the VM" "vm true" "${1:-180}"
}

# Step 0: the VM managed by ifupdown, as the installer leaves a machine.
make_ifupdown() {
    local remove=$1
    vm "sudo tee /root/zendiloos-to-ifupdown.sh >/dev/null && sudo chmod 700 /root/zendiloos-to-ifupdown.sh" \
        <"$work/to-ifupdown.sh"
    vm "sudo systemctl reset-failed zendiloos-to-ifupdown.service 2>/dev/null || true
        sudo systemd-run --quiet --collect --unit=zendiloos-to-ifupdown --setenv=REMOVE_RESOLVED=$remove \
            --property=Type=oneshot --no-block /root/zendiloos-to-ifupdown.sh"
    sleep 5
    local done=
    for _ in $(seq 1 300); do
        if vm 'sudo test -e /root/zendiloos-ifupdown.done' 2>/dev/null; then done=1; break; fi
        if vm 'systemctl is-failed --quiet zendiloos-to-ifupdown.service' 2>/dev/null; then break; fi
        sleep 2
    done
    [[ -n $done ]] || fail "the VM did not become an ifupdown machine: $(vm 'sudo tail -20 /root/zendiloos-to-ifupdown.log')"
    card=$(vm 'sudo cat /root/zendiloos-card')
    address=$(vm 'sudo cat /root/zendiloos-address')
    vm "systemctl is-active --quiet networking.service && ! systemctl is-active --quiet systemd-networkd.service" \
        || fail "networking.service is not the one running"
    vm 'sudo cat /etc/network/interfaces' >"$work/interfaces"
    vm 'cat /etc/resolv.conf' >"$work/resolv.conf"
    wait_for "the API" "curl -sfk --max-time 5 $api/health" 120
    sign_in || fail "cannot sign in"
    check "ifupdown manages $card ($address); $(vm 'pgrep -a dhcpcd || pgrep -a dhclient' | head -1 | cut -d' ' -f2-)"
}

# zendiloOS's view of a machine managed by ifupdown, and its plan.
expect_offer() {
    local setup
    setup=$(call GET /network/setup)
    jq -e '.managed == false and .manager == "ifupdown"' <<<"$setup" >/dev/null \
        || fail "zendiloOS does not see ifupdown: $setup"
    jq -e --arg card "$card" --arg address "$address" \
        '.takeover.refused == [] and .takeover.ifupdown_running
         and .takeover.interfaces == [{name: $card, ipv4: {mode: "dhcp", keep: $address}, ipv6: "automatic", dns: []}]' \
        <<<"$setup" >/dev/null || fail "the plan: $(jq -c .takeover <<<"$setup")"
    check "offered: $card by DHCP, asking for $address$(jq -r 'if .takeover.installs_resolved then ", systemd-resolved installed first" else "" end' <<<"$setup")"
}

# A job's state; nothing while the API cannot be reached.
job_state() { call GET "/jobs/$1" | jq -r '.state // empty' 2>/dev/null; }

# Starts the takeover and waits until it waits for confirmation.
take_over() {
    local job id state
    job=$(call POST /network/takeover)
    id=$(jq -er .id <<<"$job") || fail "no takeover: $job"
    check "the job $id runs"
    # The card is gone for a moment: the API with it.
    wait_for "the takeover's job" "[[ \$(job_state $id) =~ ^(succeeded|failed|interrupted)\$ ]]" 600
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "the takeover: $state"
    wait_for "the takeover to wait for confirmation" "call GET /network/setup | jq -e .pending.takeover" 60
    check "taken over; waits for confirmation until $(date -d "@$(call GET /network/setup | jq -r .pending.revert_at)" +%T)"
}

# What the agent wrote, while the takeover waits or once kept.
expect_networkd() {
    local file=/etc/systemd/network/05-zendiloos-$card.network
    vm "sudo grep -qx 'DHCP=ipv4' $file && sudo grep -qx 'ClientIdentifier=mac' $file \
        && sudo grep -qx 'RequestAddress=$address' $file" || fail "the file: $(vm "sudo cat $file")"
    check "$file: DHCP by the MAC address, asking for $address"
    vm "grep -qx 'iface lo inet loopback' /etc/network/interfaces && ! grep -q '$card' /etc/network/interfaces" \
        || fail "ifupdown kept more than the loopback: $(vm 'cat /etc/network/interfaces')"
    vm 'sudo cat /etc/network/interfaces.zendiloos-previous' | diff -u "$work/interfaces" - \
        || fail "interfaces.zendiloos-previous is not the original"
    check "ifupdown has the loopback only; the original is in interfaces.zendiloos-previous"
    [[ $(vm 'readlink /etc/resolv.conf') == ../run/systemd/resolve/stub-resolv.conf ]] \
        || fail "resolv.conf: $(vm 'ls -l /etc/resolv.conf')"
    check "/etc/resolv.conf leads to systemd-resolved's stub"
    vm 'systemctl is-active --quiet systemd-networkd.service && systemctl is-active --quiet systemd-resolved.service' \
        || fail "systemd-networkd or systemd-resolved does not run"
    vm '! pgrep -x dhcpcd && ! pgrep -x dhclient' >/dev/null || fail "ifupdown's DHCP client still runs"
    check "systemd-networkd and systemd-resolved run, ifupdown's DHCP client does not"
}

# ifupdown has everything back, as step 0 left it.
expect_ifupdown_back() {
    vm 'sudo test ! -e /etc/network/zendiloos-takeover.json' || fail "the takeover's record is still there"
    vm 'sudo cat /etc/network/interfaces' | diff -u "$work/interfaces" - || fail "/etc/network/interfaces is not back"
    [[ -z $(vm 'readlink /etc/resolv.conf || true') ]] || fail "resolv.conf is still a link"
    vm 'cat /etc/resolv.conf' | diff -u "$work/resolv.conf" - || fail "/etc/resolv.conf is not back"
    vm "test ! -e /etc/systemd/network/05-zendiloos-$card.network \
        && test ! -e /etc/network/interfaces.zendiloos-previous" || fail "the takeover's files are still there"
    check "/etc/network/interfaces and /etc/resolv.conf are back, zendiloOS's files gone"
    vm '! systemctl is-active --quiet systemd-networkd.service && ! systemctl is-active --quiet systemd-resolved.service &&
        ! systemctl is-enabled --quiet systemd-networkd.service && ! systemctl is-enabled --quiet systemd-resolved.service' \
        || fail "systemd-networkd or systemd-resolved still runs or is enabled"
    vm 'systemctl is-active --quiet networking.service' || fail "networking.service does not run"
    check "systemd-networkd and systemd-resolved off and disabled, networking.service runs"
    wait_for "the card's address from ifupdown" "vm \"ip -4 -o addr show dev $card | grep -q 'inet $address/'\"" 60
    check "$card has $address again, from $(vm 'pgrep -a dhcpcd || pgrep -a dhclient' | head -1 | cut -d' ' -f2)"
    expect_offer
}

say "Signing in (VM $ZENDILOOS_VM)"
wait_for_ssh
vm 'dpkg -s zendiloos >/dev/null 2>&1' || fail "zendiloos is not installed: ZENDILOOS_VM=$ZENDILOOS_VM just vm install"
wait_for "the API" "curl -sfk --max-time 5 $api/health" 120
if curl -sk "$api/setup" | jq -e .required >/dev/null; then
    code=$(vm 'sudo cat /var/lib/zendiloos/setup-code 2>/dev/null || true')
    curl -sfk -o /dev/null -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg code "$code" --arg name "$user" --arg password "$password" \
            '{name: $name, password: $password} + (if $code == "" then {} else {setup_code: $code} end)')" \
        "$api/setup" || fail "setup"
    check "the administrator $user set up"
fi
sign_in || fail "cannot sign in as $user"

say "0. The VM as Debian's installer leaves a machine: ifupdown"
remove=1
[[ ${ZENDILOOS_TAKEOVER_KEEP_RESOLVED:-} == 1 ]] && remove=0
make_ifupdown "$remove"
expect_offer

say "1. Taken over and confirmed"
take_over
expect_networkd
call POST /network/confirm >/dev/null
[[ $(call GET /network/setup | jq -c '[.managed, .manager, .pending]') == '[true,"networkd",null]' ]] \
    || fail "not kept: $(call GET /network/setup)"
vm 'sudo test ! -e /etc/network/zendiloos-takeover.json' || fail "the record stayed after confirming"
vm 'sudo test -e /etc/network/interfaces.zendiloos-previous && sudo test -e /etc/resolv.conf.zendiloos-previous' \
    || fail "the originals were not kept"
check "confirmed: the record went, the originals stay"
vm "ip -4 -o addr show dev $card | grep -q 'inet $address/'" || fail "$card lost $address: $(vm "ip -4 -o addr show dev $card")"
vm "networkctl status $card --no-pager | grep -q 'Network File: /etc/systemd/network/05-zendiloos-$card.network'" \
    || fail "systemd-networkd does not use zendiloOS's file: $(vm "networkctl status $card --no-pager")"
check "$card keeps $address, set up by systemd-networkd from zendiloOS's file"
lease=$(vm "sudo cat /run/systemd/netif/leases/\$(cat /sys/class/net/$card/ifindex) 2>/dev/null | grep -E '^(ADDRESS|CLIENTID)=' | tr '\n' ' '" || true)
[[ -n $lease ]] && check "the lease: $lease"
vm 'getent ahosts deb.debian.org >/dev/null' || fail "names do not resolve: $(vm 'resolvectl status --no-pager')"
servers=$(call GET /network | jq -r '.name_servers | join(", ")')
[[ -n $servers ]] || fail "zendiloOS sees no name servers"
check "names resolve through systemd-resolved (name servers $servers)"
dhcp='{"ipv4":{"mode":"dhcp"},"ipv6":"automatic","dns":[]}'
[[ $(call PUT "/network/interfaces/$card" "$dhcp" | jq -r .pending.interface) == "$card" ]] \
    || fail "the card cannot be changed now"
wait_for "the API after the change" "call GET /network/setup | jq -e .pending" 60
call POST /network/confirm >/dev/null
check "the card can be changed now, with confirmation"

say "2. Taken over and not confirmed: ifupdown gets the network back by itself"
make_ifupdown 0
expect_offer
take_over
expect_networkd
echo "   waiting for the time to confirm to run out (two minutes)"
wait_for "ifupdown to get the network back" "call GET /network/setup | jq -e '.manager == \"ifupdown\"'" 240
journal=$(vm "sudo journalctl -u zendiloos-agent --since '-5 min' --no-pager")
grep -q "the takeover was not confirmed in time" <<<"$journal" || fail "the agent did not say why"
check "the agent says the takeover was not confirmed in time"
expect_ifupdown_back

say "3. Taken over, and the agent restarted before it is confirmed"
take_over
vm 'sudo systemctl restart zendiloos-agent.service'
check "the agent restarted"
wait_for "ifupdown to get the network back" "call GET /network/setup | jq -e '.manager == \"ifupdown\"'" 120
journal=$(vm "sudo journalctl -u zendiloos-agent --since '-3 min' --no-pager")
grep -q "ifupdown gets the network back" <<<"$journal" || fail "the agent did not give it back as it started"
check "the agent gave the network back as it started"
expect_ifupdown_back

say "Passed: taken over and kept once confirmed, given back to ifupdown when not (the VM is left with ifupdown)"
