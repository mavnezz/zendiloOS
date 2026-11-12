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
#   5. while a change that closes the project's port waits, a connection to
#      that port opened before it carries nothing (it is forwarded to the
#      container, past the input chain); never confirmed, the change goes
#      back by itself
#   6. a Docker network made after the rules were written, its bridge named
#      otherwise (com.docker.network.bridge.name): the client routing to its
#      containers' addresses reaches none of them at once, though the rules
#      never name that bridge, while the containers reach each other
#   7. rules that would shut out this browser are refused
#   8. a change not confirmed goes back by itself after 90 seconds
#   9. the rules are in force again after a reboot, before the network
#
# The client is a network namespace inside the VM (192.168.77.2), joined
# to the VM by a veth pair (192.168.77.1 on fwtest0): its connections reach
# the NAS through the firewall's input chain, and those to the project's
# published port through Docker's forwarding, as another machine's would.
# The host's own requests (API, SSH) come from QEMU's gateway (10.0.2.2) and
# are always allowed by the rules here. The project needs Docker; without
# it, those steps are skipped. Step 6 reaches the container that publishes a
# port at its own address: Docker's rules let that through (before Docker
# 28, which drops such connections itself; Debian 13 has 26), so only the
# firewall's drop stops it. Steps 5 and 8 wait 90 seconds each. Everything
# is put back as it was: the firewall off, the namespace, the project and
# the containers gone. Runs in the second VM by default (the first holds a
# LAN test); account and password as for vm-storage-test.sh.
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
# Step 6's network: a bridge not named as Docker names its own.
custom_bridge=fwcustom0
custom_net=10.250.77
custom_port=18091

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
# NAS (or of ADDRESS) within three seconds.
reaches() {
    vm "sudo timeout 3 ip netns exec fwtest bash -c 'exec 3<>/dev/tcp/${2:-$nas}/$1'" >/dev/null 2>&1
}

# Whether the forward chain lets connections opened before pass at once, as
# it does with the rules kept (not while a change waits).
established_pass() {
    local chain
    chain=$(vm "sudo nft list chain inet zendiloos_firewall forward")
    [[ $chain == *"ct state established,related accept"* ]]
}

custom_down() {
    vm "sudo docker rm -f fwtest-a fwtest-b; sudo docker network rm fwcustom" >/dev/null 2>&1 || true
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
        # By the project's label: the reboot empties /tmp and its compose.yaml.
        vm "sudo docker ps -aq --filter label=com.docker.compose.project=fwtest | xargs -r sudo docker rm -f;
            sudo docker network rm fwtest_default; sudo rm -rf /tmp/fwtest" >/dev/null 2>&1 || true
        custom_down
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

say "5. A connection opened before a change that closes the project's port carries nothing while it waits"
if [[ $docker == yes ]]; then
    # One connection from the client to the project's port, held open: a
    # HEAD request at once, another once /tmp/fwtest/go is there; the status
    # line of each answer goes to a file, `none` for no answer in 5 seconds.
    vm "sudo rm -f /tmp/fwtest/go /tmp/fwtest/first /tmp/fwtest/second && cat > /tmp/fwtest/held.sh" <<EOF
exec 3<>/dev/tcp/$nas/$web_port || exit 1
answer() {
    printf 'HEAD / HTTP/1.1\r\nHost: fwtest\r\n\r\n' >&3
    local status line
    IFS= read -r -t 5 status <&3 || { echo none; return; }
    while IFS= read -r -t 5 line <&3 && [[ \$line != \$'\r' ]]; do :; done
    echo "\${status%\$'\r'}"
}
answer >/tmp/fwtest/first
for _ in \$(seq 1 300); do [[ -e /tmp/fwtest/go ]] && break; sleep 0.2; done
answer >/tmp/fwtest/second
EOF
    vm "sudo setsid -f ip netns exec fwtest bash /tmp/fwtest/held.sh </dev/null >/dev/null 2>&1"
    wait_for "the open connection's first answer" "vm 'grep -q \" 200 \" /tmp/fwtest/first'" 15
    established_pass || fail "the rules kept do not let connections opened before pass"
    # The project's rule left out: only the fallback, deny, is left for it.
    reply=$(call PUT /security/firewall "$(settings true deny "$smb_rule")")
    [[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying: $reply"
    established_pass && fail "connections opened before pass the forward chain while the change waits"
    vm "touch /tmp/fwtest/go"
    wait_for "the open connection's second try" "vm 'test -s /tmp/fwtest/second'" 15
    second=$(vm 'cat /tmp/fwtest/second')
    [[ $second == none ]] || fail "the connection opened before carried an answer while the change waits: $second"
    reaches $web_port && fail "a new connection reaches the port the change closes"
    say "   never confirmed: waiting 90 seconds for it to go back"
    wait_for "the rules from before" "reaches $web_port" 120
    setup=$(body GET /security/firewall)
    [[ $(jq -r .pending <<<"$setup") == null ]] || fail "still waiting: $setup"
    [[ $(jq -r '.current.rules | map(.service) | index("project")' <<<"$setup") != null ]] \
        || fail "the kept rules changed: $setup"
    established_pass || fail "back to the rules kept, connections opened before do not pass"
fi

say "6. A Docker network made since, its bridge named otherwise, is kept from outside at once"
if [[ $docker == yes ]]; then
    started=$(date +%s)
    vm "sudo docker network create -o com.docker.network.bridge.name=$custom_bridge \
            --subnet $custom_net.0/24 fwcustom >/dev/null &&
        sudo docker run -d --name fwtest-a --network fwcustom --ip $custom_net.10 -p $custom_port:80 \
            nginx:alpine >/dev/null &&
        sudo docker run -d --name fwtest-b --network fwcustom --ip $custom_net.11 nginx:alpine >/dev/null" \
        || fail "cannot start the containers on $custom_bridge"
    # Both serve: the NAS itself reaches them, and they reach each other.
    wait_for "the containers on $custom_bridge" \
        "vm 'timeout 3 bash -c \"exec 3<>/dev/tcp/$custom_net.10/80 && exec 4<>/dev/tcp/$custom_net.11/80\"'" 30
    vm "sudo docker exec fwtest-b wget -q -O /dev/null -T 3 http://fwtest-a/" \
        || fail "the containers on $custom_bridge do not reach each other"
    # The client routes to them through the NAS (its default route).
    reaches 80 $custom_net.10 && fail "the client reaches the container publishing a port, at its own address"
    reaches 80 $custom_net.11 && fail "the client reaches a container that publishes nothing"
    (( $(date +%s) - started < 30 )) || say "   (that took more than the 30 seconds between the firewall's refreshes)"
    rules=$(vm "sudo nft list table inet zendiloos_firewall")
    [[ $rules == *"$custom_bridge"* ]] && fail "the rules name $custom_bridge: the drop must not need its name"
    vm "sudo docker exec fwtest-a wget -q -O /dev/null -T 3 http://fwtest-b/" \
        || fail "the containers on $custom_bridge no longer reach each other"
    custom_down
fi

say "7. Rules that would shut out this browser are refused"
locked=$(jq -cn '{enabled: true, fallback: "deny", rules: []}')
reply=$(call PUT /security/firewall "$locked")
[[ $(tail -1 <<<"$reply") == 409 && $reply == *would_lock_out* ]] || fail "not refused: $reply"

say "8. A change not confirmed goes back by itself"
reply=$(call PUT /security/firewall "$(settings true deny)")
[[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying: $reply"
reaches 445 && fail "the unconfirmed change is not in force"
say "   waiting 90 seconds"
wait_for "the rules from before" "reaches 445" 120
setup=$(body GET /security/firewall)
[[ $(jq -r .pending <<<"$setup") == null ]] || fail "still waiting: $setup"
[[ $(jq -r '.current.rules | map(.service) | index("smb")' <<<"$setup") != null ]] || fail "the kept rules changed"

say "9. After a reboot the rules are in force before the network"
# Waits for a new boot, not only for SSH: a guest VM that ignores the
# shutdown keeps libvirt-guests waiting up to five minutes, with the old
# system still answering meanwhile.
boot=$(vm 'cat /proc/sys/kernel/random/boot_id')
booted_anew() {
    local now
    now=$(vm 'cat /proc/sys/kernel/random/boot_id' 2>/dev/null) || return 1
    [[ -n $now && $now != "$boot" ]]
}
vm 'sudo systemctl reboot' || true
sleep 10
wait_for "the VM after the reboot" booted_anew 600
[[ $(vm 'systemctl is-active zendiloos-firewall.service') == active ]] || fail "the unit is not active"
vm 'sudo nft list table inet zendiloos_firewall' | grep -q "saddr $client" || fail "the rules are not back"
before=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value zendiloos-firewall.service")
network=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value network-pre.target")
(( before <= network )) || fail "the firewall came after the network ($before > $network)"
client_up
wait_for "the web interface after the reboot" "sign_in" 120
reaches 445 || fail "SMB from the client is blocked after the reboot"

say "All firewall tests passed."
