#!/usr/bin/env bash
# Tests the firewall end to end in a test VM, through the HTTPS API and the
# real nftables:
#
#   1. turned on with "allow" when no rule matches, nothing changes
#   2. "deny" blocks SMB and a Docker project's published port, while the
#      web interface (and SSH, which this script needs) stay reachable; the
#      blocked services are listed and told in a notification
#   3. an allow rule for SMB from the client's address lets it in again
#   4. an allow rule for the project opens its published port
#   5. rules that would shut out this browser are refused
#   6. a change not confirmed goes back by itself after 90 seconds
#   7. the rules are in force again after a reboot, before the network
#
# The client is a network namespace inside the VM (192.168.77.2), joined
# to the VM by a veth pair (192.168.77.1 on fwtest0): its connections reach
# the NAS through the firewall's input chain, and those to the project's
# published port through Docker's forwarding, as another machine's would.
# The host's own requests (API, SSH) come from QEMU's gateway (10.0.2.2) and
# are always allowed by the rules here. The project needs Docker; without
# it, those steps are skipped. Everything is put back as it was: the
# firewall off, the namespace and the project gone. Runs in the second VM by
# default (the first holds a LAN test); account and password as for
# vm-storage-test.sh.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
port=${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}
base="https://127.0.0.1:$port"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
nas=192.168.77.1
client=192.168.77.2
web_port=18090

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

sign_in() {
    curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/csrf"
}

# METHOD PATH [BODY]; prints the body, then the status.
call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -m 30 -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/csrf")" -w '\n%{http_code}')
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}
status() { call "$@" | tail -1; }
body() { call "$@" | sed '$d'; }

# The rules this script always keeps first: the host reaches the web
# interface and SSH.
host_rules='[{"enabled":true,"service":"web_interface","source":"10.0.2.0/24","action":"allow"},
             {"enabled":true,"service":"ssh","source":"10.0.2.0/24","action":"allow"}]'

# Settings: enabled, fallback, and rules after the host's.
settings() {
    jq -cn --argjson enabled "$1" --arg fallback "$2" --argjson host "$host_rules" --argjson rules "${3:-[]}" \
        '{enabled: $enabled, fallback: $fallback, rules: ($host + $rules)}'
}

# Applies and keeps settings.
apply() {
    local reply
    reply=$(call PUT /security/firewall "$1")
    [[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying: $reply"
    [[ $(status POST /security/firewall/confirm) == 204 ]] || fail "confirming"
}

# Whether the client in the namespace gets a TCP connection to PORT of the
# NAS within three seconds.
reaches() {
    vm "sudo timeout 3 ip netns exec fwtest bash -c 'exec 3<>/dev/tcp/$nas/$1'" >/dev/null 2>&1
}

client_up() {
    vm "sudo bash -s" <<EOF
set -e
ip netns del fwtest 2>/dev/null || true
ip link del fwtest0 2>/dev/null || true
ip netns add fwtest
ip link add fwtest0 type veth peer name fwtest1
ip link set fwtest1 netns fwtest
ip addr add $nas/24 dev fwtest0
ip link set fwtest0 up
ip netns exec fwtest ip addr add $client/24 dev fwtest1
ip netns exec fwtest ip link set fwtest1 up
ip netns exec fwtest ip link set lo up
ip netns exec fwtest ip route add default via $nas
EOF
}

cleanup() {
    status PUT /security/firewall "$(settings false allow)" >/dev/null 2>&1 || true
    status POST /security/firewall/confirm >/dev/null 2>&1 || true
    vm "sudo ip netns del fwtest; sudo ip link del fwtest0" >/dev/null 2>&1 || true
    if [[ ${docker:-no} == yes ]]; then
        vm "cd /tmp/fwtest && sudo docker compose -p fwtest down" >/dev/null 2>&1 || true
    fi
    rm -rf "$work"
}
trap cleanup EXIT

say "Setting up: a client namespace in the VM, a project publishing port $web_port"
sign_in || fail "cannot sign in as $user"
client_up
docker=no
if vm 'command -v docker' >/dev/null 2>&1; then
    docker=yes
    vm "mkdir -p /tmp/fwtest && cat > /tmp/fwtest/compose.yaml" <<EOF
services:
  web:
    image: nginx:alpine
    ports: ["$web_port:80"]
EOF
    vm "cd /tmp/fwtest && sudo docker compose -p fwtest up -d --quiet-pull" >/dev/null 2>&1 \
        || fail "cannot start the project"
    wait_for "the project's port" "reaches $web_port" 60
else
    say "  Docker is not installed: the project's steps are skipped"
fi
reaches 445 || fail "SMB is not reachable before the firewall is on (is SMB on?)"

say "1. On, allowing what no rule matches: nothing changes"
apply "$(settings true allow)"
vm 'sudo nft list table inet zendiloos_firewall' >/dev/null || fail "the table is not there"
reaches 445 || fail "SMB was blocked"
[[ $docker == no ]] || reaches $web_port || fail "the project's port was blocked"
[[ $(vm 'systemctl is-active zendiloos-firewall.service') == active ]] || fail "the unit is not active"

say "2. Denying the rest: SMB and the project are blocked, the web interface is not"
apply "$(settings true deny)"
reaches 445 && fail "SMB is still reachable"
[[ $docker == no ]] || ! reaches $web_port || fail "the project's port is still reachable"
[[ $(status GET /system/info) == 200 ]] || fail "the web interface is blocked"
blocked=$(body GET /security/firewall | jq -c .blocked)
[[ $blocked == *'"smb"'* ]] || fail "SMB is not listed as blocked: $blocked"
[[ $docker == no || $blocked == *'"project:fwtest"'* ]] || fail "the project is not listed: $blocked"
wait_for "the notification" \
    "body GET /notifications | jq -e '.notifications[] | select(.code == \"firewall_blocked\" and .params.service == \"smb\")'" 60

say "3. Allowing SMB from the client's address"
smb_rule='[{"enabled":true,"service":"smb","source":"'$client'","action":"allow"}]'
apply "$(settings true deny "$smb_rule")"
reaches 445 || fail "SMB is still blocked"
[[ $docker == no ]] || ! reaches $web_port || fail "the project's port opened with SMB"

say "4. Allowing the project from anyone"
if [[ $docker == yes ]]; then
    project_rule='[{"enabled":true,"service":"project","project":"fwtest","source":"any","action":"allow"}]'
    apply "$(settings true deny "$(jq -cn --argjson a "$smb_rule" --argjson b "$project_rule" '$a + $b')")"
    reaches $web_port || fail "the project's port is still blocked"
    vm "sudo nft list chain inet zendiloos_firewall published" | grep -q "ct original proto-dst" \
        || fail "no rule for the published port"
fi

say "5. Rules that would shut out this browser are refused"
locked=$(jq -cn '{enabled: true, fallback: "deny", rules: []}')
reply=$(call PUT /security/firewall "$locked")
[[ $(tail -1 <<<"$reply") == 409 && $reply == *would_lock_out* ]] || fail "not refused: $reply"

say "6. A change not confirmed goes back by itself"
reply=$(call PUT /security/firewall "$(settings true deny)")
[[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying: $reply"
reaches 445 && fail "the unconfirmed change is not in force"
say "   waiting 90 seconds"
wait_for "the rules from before" "reaches 445" 120
setup=$(body GET /security/firewall)
[[ $(jq -r .pending <<<"$setup") == null ]] || fail "still waiting: $setup"
[[ $(jq -r '.current.rules | map(.service) | index("smb")' <<<"$setup") != null ]] || fail "the kept rules changed"

say "7. After a reboot the rules are in force before the network"
vm 'sudo systemctl reboot' || true
sleep 10
wait_for "the VM after the reboot" "vm true" 300
[[ $(vm 'systemctl is-active zendiloos-firewall.service') == active ]] || fail "the unit is not active"
vm 'sudo nft list table inet zendiloos_firewall' | grep -q "saddr $client" || fail "the rules are not back"
before=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value zendiloos-firewall.service")
network=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value network-pre.target")
(( before <= network )) || fail "the firewall came after the network ($before > $network)"
client_up
wait_for "the web interface after the reboot" "sign_in" 120
reaches 445 || fail "SMB from the client is blocked after the reboot"

say "All firewall tests passed."
