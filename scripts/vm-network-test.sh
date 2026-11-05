#!/usr/bin/env bash
# Tests changing the network in a test VM, through the HTTPS API:
#
#   1. the card's own address, made fixed and confirmed, stays
#   2. an address that cuts the VM off is not confirmed: after the time to
#      confirm, the confirmed settings return and the VM is reachable again
#   3. back to DHCP, confirmed, and zendiloOS's file removed
#
# Run it against a VM that nothing else needs: ZENDILOOS_VM=2 by default.
# Needs the package installed and set up; account and password come from
# ZENDILOOS_USER and ZENDILOOS_PASSWORD (defaults: admin, "correct horse
# battery").
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk --max-time 10 -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

say "Signing in as $user (VM $ZENDILOOS_VM)"
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in"

[[ $(call GET /network/setup | jq .managed) == true ]] || fail "systemd-networkd does not manage the VM's network"
network=$(call GET /network)
card=$(jq -r '.default_routes[0].interface' <<<"$network")
address=$(jq -r --arg card "$card" '.interfaces[] | select(.name == $card) | .addresses[] | select(.address | contains(".")) | .address' <<<"$network" | head -1)
prefix=$(jq -r --arg card "$card" '.interfaces[] | select(.name == $card) | .addresses[] | select(.address | contains(".")) | .prefix' <<<"$network" | head -1)
gateway=$(jq -r '.default_routes[0].gateway' <<<"$network")
echo "   $card: $address/$prefix via $gateway"
file="/etc/systemd/network/05-zendiloos-$card.network"

say "1. The card's own address, fixed and confirmed"
fixed=$(jq -n --arg a "$address" --argjson p "$prefix" --arg g "$gateway" \
    '{ipv4: {mode: "static", address: $a, prefix: $p, gateway: $g}, ipv6: "automatic", dns: ["10.0.2.3"]}')
applied=$(call PUT "/network/interfaces/$card" "$fixed")
[[ $(jq -r '.pending.interface' <<<"$applied") == "$card" ]] || fail "apply: $applied"
[[ $(call PUT "/network/interfaces/$card" "$fixed" | jq -r .code) == change_pending ]] || fail "a second change"
wait_for "the API at the same address" "call GET /network/setup | jq -e .pending" 30
call POST /network/confirm >/dev/null
[[ $(call GET /network/setup | jq .pending) == null ]] || fail "still pending after confirming"
vm "sudo grep -q 'Address=$address/$prefix' $file && sudo test ! -e $file.previous" || fail "the file: $(vm "sudo cat $file")"
vm "networkctl status $card --no-pager | grep -q 'Network File: $file'" || fail "networkd does not use the file"

say "2. An address that cuts the VM off goes back by itself"
cut=$(jq -n '{ipv4: {mode: "static", address: "10.99.99.99", prefix: 24, gateway: null}, ipv6: "off", dns: []}')
call PUT "/network/interfaces/$card" "$cut" >/dev/null || true
sleep 5
! call GET /network/setup >/dev/null 2>&1 || fail "the VM is still reachable at its old address"
echo "   cut off; waiting for the old settings (two minutes)"
wait_for "the VM to come back" "call GET /network/setup | jq -e '.pending == null'" 240
vm "sudo grep -q 'Address=$address/$prefix' $file && sudo test ! -e $file.previous" || fail "not the confirmed settings: $(vm "sudo cat $file")"
journal=$(vm "sudo journalctl -u zendiloos-agent --since '-5 min' --no-pager")
grep -q "not confirmed in time" <<<"$journal" || fail "the agent did not say why"

say "3. Back to DHCP"
call PUT "/network/interfaces/$card" '{"ipv4":{"mode":"dhcp"},"ipv6":"automatic","dns":[]}' >/dev/null
wait_for "the API after DHCP" "call GET /network/setup | jq -e .pending" 60
call POST /network/confirm >/dev/null
vm "sudo rm -f $file && sudo networkctl reload && sudo networkctl reconfigure $card"
wait_for "the API with the image's own settings" "call GET /network/setup | jq -e .managed" 60

say "Passed: confirmed settings stay, unconfirmed ones go back by themselves"
