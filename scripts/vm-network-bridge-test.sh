#!/usr/bin/env bash
# Tests bridges, bonds and VLANs in a throwaway test VM, through the HTTPS
# API and the real systemd-networkd, iptables and nftables:
#
#   0. systemd-networkd manages the VM's network (taken over from ifupdown
#      first, should an earlier takeover test have left it so); the
#      firewall is on (allow when no rule matches, so nothing changes) and
#      its table is noted, to be the same at the end
#   1. a bridge over the VM's card, planned, made and confirmed: the NAS
#      keeps its address (the bridge has the card's MAC address, so QEMU's
#      DHCP hands it out again), the API answers through it
#   2. a bridge over a spare card, not confirmed: two minutes later its
#      files are gone, its link too (systemd-networkd never deletes one),
#      the card's file is as before; once more, with the agent restarted
#      before it is confirmed: the same as the agent starts
#   3. with Docker installed through the App Center (and a pool for its
#      data made first, should there be none): br_netfilter is loaded and
#      FORWARD's policy is DROP, as Docker leaves them; a stand-in for a VM
#      (a network namespace on a veth pair in the bridge) gets an address
#      from QEMU's DHCP and reaches its gateway through the bridge; without
#      zendiloOS's jump in DOCKER-USER it reaches nothing, and the jump is
#      back after Docker restarts. With ZENDILOOS_BRIDGE_TEST_VM=1 and
#      /dev/kvm in the VM, also a real VM of the VMs plugin: the wizard
#      offers the bridge, the bridge cannot go while the VM is on it, and
#      the DHCP server's offer reaches the VM's tap as its firmware boots
#      from the network
#   4. a bond of the two spare cards (active-backup) and a VLAN on it, made
#      and confirmed: both come up, the bond with both members, the VLAN
#      with its id and address; then the VLAN and the bond removed
#   5. the bridge removed: the card has its settings and address back, and
#      zendiloOS's chain is gone from Docker's tables
#   6. the firewall's table as noted in step 0, Docker's tables as Docker
#      made them
#
# The VM is reached through QEMU's user networking (SSH and HTTPS forwarded
# to 10.0.2.15); the spare cards are on a network of their own. Make the VM
# with them, and nothing else on it that matters:
#   ZENDILOOS_VM=3 just vm create
#   ZENDILOOS_VM=3 ZENDILOOS_VM_EXTRA_NICS=2 just vm start
#   ZENDILOOS_VM=3 just vm install
#   just vm-network-bridge-test
#   ZENDILOOS_VM=3 just vm destroy
# The package must be installed; an administrator is set up if there is
# none (ZENDILOOS_USER and ZENDILOOS_PASSWORD, defaults admin and "correct
# horse battery"). Docker, a pool for it and busybox (the stand-in's DHCP
# client) are installed if missing, which needs the internet. Takes about
# fifteen minutes; the VM is left with the network as it found it (and
# Docker installed).
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-3}
[[ $ZENDILOOS_VM != 1 ]] || { echo "not on VM 1: it holds the LAN test (ZENDILOOS_VM=3)" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
bridge=zbr0
spare_bridge=zbr9
bond=zbond0
vlan=zbond0.42
vlan_address=192.168.42.2
firewall_was_on=
netns=zvm

say() { printf '\033[1m%s\033[0m\n' "$*"; }
check() { printf '   ✓ %s\n' "$*"; }
note() { printf '   · %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# METHOD PATH [BODY]: the body in $work/last, the status printed.
status() {
    local method=$1 path=$2 body=${3:-}
    rm -f "$work/last"
    local args=(-sk --max-time 20 -o "$work/last" -w '%{http_code}' -b "$work/jar" -c "$work/jar" -X "$method"
                -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path" || echo 000
}

# METHOD PATH [BODY]: the body printed; nothing if there is no answer.
call() {
    status "$@" >/dev/null
    cat "$work/last" 2>/dev/null || true
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

job_succeeded() {
    local id=$1 seconds=${2:-1800} state
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) =~ ^(succeeded|failed|interrupted)\$ ]]" "$seconds"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

# The change waiting, `null` for none, nothing while the API cannot be
# reached.
pending() { call GET /network/setup | jq -c .pending 2>/dev/null || true; }
waiting() { [[ $(pending) == '{'* ]]; }

# Sends a change of the network, which may cut the request's connection
# off for a moment, and waits until it waits for confirmation.
change() {
    local method=$1 path=$2 body=${3:-} code
    code=$(status "$method" "$path" "$body")
    case $code in
        202) ;;
        000) note "the reply was lost while the network changed; looking again" ;;
        *) fail "$method $path: $code $(cat "$work/last")" ;;
    esac
    wait_for "the change to wait for confirmation" waiting 90
    check "applied, waits for confirmation: $(pending)"
}

confirm() {
    [[ $(status POST /network/confirm) == 204 ]] || fail "confirming: $(cat "$work/last")"
    [[ $(pending) == null ]] || fail "still pending after confirming"
    check "confirmed"
}

# Waits for the time to confirm to run out, and the change to go back.
lapse() {
    note "waiting for the time to confirm to run out (two minutes)"
    wait_for "the change to go back" "[[ \$(pending) == null ]]" 240
    vm 'sudo test ! -e /etc/systemd/network/zendiloos-change.json' || fail "the change's record stayed"
    check "gone back by itself"
}

link_exists() { vm "ip link show dev $1" >/dev/null 2>&1; }
address_of() { vm "ip -4 -o addr show dev $1 | awk '{print \$4}' | cut -d/ -f1 | head -1"; }
# The VM has no jq; its answer is read here.
master_of() { vm "ip -j link show dev $1" | jq -r '.[0].master // empty'; }
mac_of() { vm "cat /sys/class/net/$1/address"; }
firewall_table() { vm 'sudo nft list table inet zendiloos_firewall 2>/dev/null || true'; }
# Docker's tables without zendiloOS's chain, sorted: a restart of Docker
# may write the same rules in another order.
docker_tables() {
    vm "{ sudo iptables -S; sudo ip6tables -S; } 2>/dev/null | grep -v ZENDILOOS-BRIDGES | sort" || true
}

cleanup() {
    set +e
    vm "sudo ip netns del $netns; sudo ip link del ${netns}h" >/dev/null 2>&1
    if [[ -n ${vm_id:-} ]]; then
        status POST "/vms/$vm_id/force-off" >/dev/null
        status POST "/vms/$vm_id/delete" '{"confirm":"zbridged"}' >/dev/null
    fi
    if [[ $firewall_was_on == no ]]; then
        status PUT /security/firewall '{"enabled":false,"fallback":"allow","rules":[]}' >/dev/null
        status POST /security/firewall/confirm >/dev/null
    fi
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in (VM $ZENDILOOS_VM)"
wait_for "SSH to the VM" "vm true" 180
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

say "0. systemd-networkd manages the network; the firewall is on"
[[ $(pending) == null ]] || fail "a change waits already: $(pending)"
setup=$(call GET /network/setup)
if [[ $(jq -r .manager <<<"$setup") == ifupdown ]]; then
    note "ifupdown manages the network (an earlier takeover test): taking it over"
    job=$(call POST /network/takeover | jq -er .id) || fail "no takeover: $(cat "$work/last")"
    wait_for "the takeover" "[[ \$(call GET /jobs/$job | jq -r .state) =~ ^(succeeded|failed)\$ ]]" 600
    wait_for "the takeover to wait" waiting 60
    confirm
    setup=$(call GET /network/setup)
fi
jq -e '.managed' <<<"$setup" >/dev/null || fail "systemd-networkd does not manage the network: $setup"
[[ $(jq '.devices | length' <<<"$setup") == 0 ]] \
    || fail "devices left from an earlier run: $(jq -c '[.devices[].name]' <<<"$setup"); remove them first"
network=$(call GET /network)
card=$(jq -r '.default_routes[] | select(.gateway | contains(".")) | .interface' <<<"$network" | head -1)
address=$(address_of "$card")
card_mac=$(mac_of "$card")
[[ -n $card && -n $address ]] || fail "no card with the default route: $network"
read -r spare1 spare2 < <(jq -r --arg card "$card" \
    '[.interfaces[] | select(.kind == "ethernet" and .name != $card and .member_of == null) | .name] | .[:2] | join(" ")' \
    <<<"$network"; echo)
[[ -n ${spare2:-} ]] || fail "two spare cards are needed: start the VM with ZENDILOOS_VM_EXTRA_NICS=2"
spare1_mac=$(mac_of "$spare1")
check "$card ($address, $card_mac) carries the default route; $spare1 and $spare2 are spare"
vm 'sudo test -e /usr/lib/systemd/system/zendiloos-docker-bridges.service' || fail "the package has no zendiloos-docker-bridges.service"
spare_file_before=$(vm "sudo cat /etc/systemd/network/05-zendiloos-$spare1.network 2>/dev/null || echo none")
if call GET /security/firewall | jq -e '.current.enabled' >/dev/null; then
    firewall_was_on=yes
    check "the firewall is on"
else
    firewall_was_on=no
    rules='[{"enabled":true,"service":"web_interface","source":"10.0.2.0/24","action":"allow"},
            {"enabled":true,"service":"ssh","source":"10.0.2.0/24","action":"allow"}]'
    [[ $(status PUT /security/firewall "$(jq -cn --argjson r "$rules" '{enabled: true, fallback: "allow", rules: $r}')") == 202 ]] \
        || fail "the firewall: $(cat "$work/last")"
    [[ $(status POST /security/firewall/confirm) == 204 ]] || fail "confirming the firewall"
    check "the firewall turned on (allow when no rule matches)"
fi
wait_for "the firewall's table" "vm 'sudo nft list table inet zendiloos_firewall'" 60
firewall_before=$(firewall_table)
check "the firewall's table noted ($(wc -l <<<"$firewall_before") lines)"

say "1. A bridge over $card, made and confirmed"
plan=$(call POST /network/devices/plan "$(jq -cn --arg b "$bridge" --arg c "$card" \
    '{action: "make", device: {kind: "bridge", name: $b, members: [$c]}}')")
jq -e --arg c "$card" --arg a "$address" --arg mac "$card_mac" \
    '.device.mac == $mac and (.interfaces[] | select(.interface == $c) | .becomes.moved)
     and ([.interfaces[] | select(.becomes.fate == "settings")][0].becomes.settings.ipv4.keep == $a)' \
    <<<"$plan" >/dev/null || fail "the plan: $plan"
check "planned: $card's settings and MAC address go to $bridge, which asks for $address"
change POST /network/devices "$(jq -cn --arg b "$bridge" --arg c "$card" '{kind: "bridge", name: $b, members: [$c]}')"
wait_for "$bridge to have $address" "[[ \$(address_of $bridge) == $address ]]" 60
confirm
[[ $(master_of "$card") == "$bridge" ]] || fail "$card is not in $bridge"
[[ $(mac_of "$bridge") == "$card_mac" ]] || fail "$bridge has $(mac_of "$bridge"), not $card_mac"
[[ -z $(address_of "$card") ]] || fail "$card kept an address"
check "$bridge has $address and $card_mac; $card is a port of it without an address"
vm "sudo grep -qx 'MACAddress=$card_mac' /etc/systemd/network/05-zendiloos-$bridge.netdev \
    && sudo grep -qx 'Bridge=$bridge' /etc/systemd/network/05-zendiloos-$card.network \
    && sudo grep -qx 'RequestAddress=$address' /etc/systemd/network/05-zendiloos-$bridge.network" \
    || fail "the files: $(vm "sudo head -50 /etc/systemd/network/05-zendiloos-*")"
vm "networkctl status $bridge --no-pager | grep -q 'Network File: /etc/systemd/network/05-zendiloos-$bridge.network'" \
    || fail "systemd-networkd does not use the bridge's file"
check "zendiloOS's files, used by systemd-networkd"
call GET /network/setup | jq -e --arg b "$bridge" --arg c "$card" \
    '.devices == [{name: $b, kind: "bridge", members: [$c], parent: null, vlan_id: null, mode: null, stp: false, mac: .devices[0].mac}]' \
    >/dev/null || fail "the setup: $(call GET /network/setup)"
check "the API answers through $bridge and names it"

say "2. A bridge over $spare1, not confirmed"
change POST /network/devices "$(jq -cn --arg b "$spare_bridge" --arg c "$spare1" '{kind: "bridge", name: $b, members: [$c]}')"
wait_for "$spare_bridge" "link_exists $spare_bridge" 30
vm 'sudo test -e /etc/systemd/network/zendiloos-change.json' || fail "no record of the change"
check "$spare_bridge is there, the change recorded"
lapse
link_exists "$spare_bridge" && fail "$spare_bridge's link stayed"
vm "sudo test ! -e /etc/systemd/network/05-zendiloos-$spare_bridge.netdev" || fail "$spare_bridge's file stayed"
[[ $(vm "sudo cat /etc/systemd/network/05-zendiloos-$spare1.network 2>/dev/null || echo none") == "$spare_file_before" ]] \
    || fail "$spare1's file is not as before"
[[ -z $(master_of "$spare1") ]] || fail "$spare1 is still in a bridge"
check "$spare_bridge's files and link gone, $spare1 as before"
journal=$(vm "sudo journalctl -u zendiloos-agent --since '-4 min' --no-pager")
grep -q "not confirmed in time" <<<"$journal" || fail "the agent did not say why"
grep -q "device deleted" <<<"$journal" || fail "the agent did not say it deleted the link"
check "the agent says it was not confirmed and deleted the link"
change POST /network/devices "$(jq -cn --arg b "$spare_bridge" --arg c "$spare1" '{kind: "bridge", name: $b, members: [$c]}')"
wait_for "$spare_bridge" "link_exists $spare_bridge" 30
vm 'sudo systemctl restart zendiloos-agent.service'
wait_for "the change to go back as the agent starts" "! link_exists $spare_bridge && [[ \$(pending) == null ]]" 90
vm "sudo test ! -e /etc/systemd/network/05-zendiloos-$spare_bridge.netdev && sudo test ! -e /etc/systemd/network/zendiloos-change.json" \
    || fail "the files stayed after the agent restarted"
check "made again and the agent restarted: gone as the agent started"

say "3. Docker, br_netfilter and a frame bridged through"
docker=$(call GET /apps/docker)
if ! jq -e .installed <<<"$docker" >/dev/null; then
    pool=$(call GET /storage/pools | jq -r '.[0].name // empty')
    if [[ -z $pool ]]; then
        disk=$(call GET /storage/disks | jq -r '[.[] | select(.usage == "unused" and (.name | startswith("sd"))
            and .id_link != null and (.read_only | not)) | .name][0] // empty')
        [[ -n $disk ]] || fail "no pool and no free disk to make one on for Docker"
        plan=$(call POST /storage/pools/plan "{\"name\":\"zbridge\",\"layout\":\"single\",\"disks\":[\"$disk\"]}")
        token=$(jq -er .token <<<"$plan") || fail "the pool's plan: $plan"
        job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"zbridge\"}" | jq -r .id)" 600
        pool=zbridge
        check "the pool $pool made on $disk, for Docker's data"
    fi
    note "installing Docker through the App Center, on $pool"
    job_succeeded "$(call POST /apps/docker/install "{\"pool\":\"$pool\"}" | jq -er .id)" 1800
fi
wait_for "Docker" "call GET /apps/docker | jq -e .running" 120
check "Docker $(call GET /apps/docker | jq -r .version) runs"
vm 'lsmod | grep -q "^br_netfilter"' || fail "br_netfilter is not loaded"
[[ $(vm 'cat /proc/sys/net/bridge/bridge-nf-call-iptables') == 1 ]] || fail "bridge-nf-call-iptables is not 1"
policy=$(vm 'sudo iptables -S FORWARD | head -1')
note "iptables: $policy; $(vm 'sudo iptables -V')"
[[ $policy == '-P FORWARD DROP' ]] || note "Docker did not set FORWARD's policy to DROP here; the frames pass anyway"
check "br_netfilter loaded, bridged frames go through iptables"
vm 'sudo iptables -S DOCKER-USER' | grep -qx -- '-A DOCKER-USER -j ZENDILOOS-BRIDGES' \
    || fail "no jump to zendiloOS's chain: $(vm 'sudo iptables -S DOCKER-USER')"
vm 'sudo iptables -S ZENDILOOS-BRIDGES' | grep -qx -- "-A ZENDILOOS-BRIDGES -i $bridge -o $bridge -j ACCEPT" \
    || fail "zendiloOS's chain: $(vm 'sudo iptables -S ZENDILOOS-BRIDGES')"
check "DOCKER-USER jumps to ZENDILOOS-BRIDGES, which accepts what stays within $bridge"
docker_before=$(docker_tables)

# A stand-in for a VM: a namespace on a veth pair, its host end a port of
# the bridge, its DHCP client busybox's.
vm 'command -v busybox >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q busybox >/dev/null' \
    || fail "busybox could not be installed"
vm "sudo bash -s" <<EOF || fail "the stand-in for a VM"
set -e
ip netns del $netns 2>/dev/null || true
ip link del ${netns}h 2>/dev/null || true
ip netns add $netns
ip link add ${netns}h type veth peer name ${netns}g
ip link set ${netns}g netns $netns
ip link set ${netns}h master $bridge up
ip netns exec $netns ip link set lo up
ip netns exec $netns ip link set ${netns}g up
EOF
dhcp=$(vm "sudo ip netns exec $netns busybox udhcpc -i ${netns}g -n -q -t 5 -s /bin/true 2>&1" || true)
lease=$(grep -oE 'lease of [0-9.]+' <<<"$dhcp" | awk '{print $3}' || true)
[[ $lease == 10.0.2.* && $lease != "$address" ]] || fail "no address from QEMU's DHCP through $bridge: $dhcp"
vm "sudo ip netns exec $netns ip addr add $lease/24 dev ${netns}g"
check "the stand-in got $lease from QEMU's DHCP, through $bridge"
reaches() { vm "sudo ip netns exec $netns ping -c 2 -W 2 10.0.2.2" >/dev/null 2>&1; }
reaches || fail "the stand-in does not reach 10.0.2.2 through $bridge"
check "it reaches QEMU's gateway 10.0.2.2 through $bridge"
if [[ $policy == '-P FORWARD DROP' ]]; then
    vm 'sudo iptables -D DOCKER-USER -j ZENDILOOS-BRIDGES'
    reaches && fail "the frames passed without zendiloOS's jump: is the test right?"
    check "without zendiloOS's jump in DOCKER-USER, Docker's FORWARD chain drops them"
    vm 'sudo systemctl start zendiloos-docker-bridges.service'
    reaches || fail "the frames do not pass again once the jump is back"
    check "zendiloos-docker-bridges.service puts the jump back"
fi
vm 'sudo systemctl restart docker.service'
wait_for "Docker after its restart" "vm 'sudo docker info >/dev/null'" 120
wait_for "the jump after Docker's restart" "vm 'sudo iptables -S DOCKER-USER' | grep -qx -- '-A DOCKER-USER -j ZENDILOOS-BRIDGES'" 60
reaches || fail "the stand-in does not reach its gateway after Docker restarted"
[[ $(vm 'sudo iptables -S DOCKER-USER' | grep -c ZENDILOOS-BRIDGES) == 1 ]] || fail "more than one jump"
check "after Docker restarted, the jump is back (once) and the frames pass"
vm "sudo nft list chain inet zendiloos_firewall forward" | grep -q 'meta oifkind "bridge"' \
    || fail "the firewall's forward chain is not in force"
check "zendiloOS's own forward chain is in force and lets them pass too"

if [[ ${ZENDILOOS_BRIDGE_TEST_VM:-} == 1 ]]; then
    if ! vm 'test -e /dev/kvm'; then
        note "no /dev/kvm in the VM (nested virtualization off): no real VM"
    else
        note "a real VM of the VMs plugin on $bridge"
        plugin=$(call GET /vms/plugin)
        if ! jq -e .installed <<<"$plugin" >/dev/null; then
            [[ $(status POST /vms/plugin/install) == 202 ]] || fail "the VMs plugin: $(cat "$work/last")"
            job_succeeded "$(jq -r .id "$work/last")" 1800
        fi
        if ! call GET /vms/plugin | jq -e .storage >/dev/null; then
            pool=$(call GET /apps/docker | jq -r '.volume.pool // empty')
            [[ $(status POST /vms/plugin/storage "{\"pool\":\"$pool\"}") == 200 ]] || fail "the VMs' storage: $(cat "$work/last")"
        fi
        call GET /vms/plugin | jq -e --arg b "$bridge" '.bridges | index($b)' >/dev/null \
            || fail "the wizard does not offer $bridge: $(call GET /vms/plugin | jq -c .bridges)"
        check "the VM wizard offers $bridge"
        new=$(jq -cn --arg b "$bridge" '{name: "zbridged", os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi",
            disk_gib: 1, network: {kind: "bridge", bridge: $b}, autostart: false}')
        [[ $(status POST /vms "$new") == 202 ]] || fail "the VM: $(cat "$work/last")"
        vm_id=$(jq -er .vm.id "$work/last")
        job_succeeded "$(jq -r .job.id "$work/last")" 300
        [[ $(status DELETE "/network/devices/$bridge") == 409 ]] && jq -e '.code == "device_in_use" and .detail == "zbridged"' "$work/last" >/dev/null \
            || fail "$bridge could be removed with a VM on it: $(cat "$work/last")"
        check "$bridge stays while the VM is on it (device_in_use: zbridged)"
        [[ $(status POST "/vms/$vm_id/start") == 200 ]] || fail "starting the VM: $(cat "$work/last")"
        tap=$(vm "sudo virsh -c qemu:///system domiflist zendiloos-$vm_id | awk 'NR > 2 && \$1 != \"\" {print \$1; exit}'")
        [[ -n $tap ]] || fail "the VM has no tap"
        # The VM's tap is a port of the bridge and up. Traffic through the
        # bridge was shown above with the stand-in: a VM with an empty disk
        # sends none, as OVMF boots only the devices in libvirt's boot order,
        # and a network card is never among them.
        wait_for "$tap to join $bridge" "[[ \$(master_of $tap) == $bridge ]]" 30
        vm "ip -br link show dev $tap | grep -qw UP" || fail "$tap is not up: $(vm "ip -br link show dev $tap")"
        check "the VM's tap $tap is a port of $bridge and up"
        status POST "/vms/$vm_id/force-off" >/dev/null
        [[ $(status POST "/vms/$vm_id/delete" '{"confirm":"zbridged"}') == 204 ]] || fail "deleting the VM: $(cat "$work/last")"
        vm_id=
        check "the VM deleted"
    fi
fi
vm "sudo ip netns del $netns; sudo ip link del ${netns}h 2>/dev/null || true"

say "4. A bond of $spare1 and $spare2 with a VLAN on it"
change POST /network/devices "$(jq -cn --arg b "$bond" --arg one "$spare1" --arg two "$spare2" \
    '{kind: "bond", name: $b, members: [$one, $two], mode: "active_backup"}')"
wait_for "$bond with both members" "vm 'grep -c \"^Slave Interface\" /proc/net/bonding/$bond' | grep -qx 2" 60
confirm
vm "grep -q 'Bonding Mode: fault-tolerance (active-backup)' /proc/net/bonding/$bond" || fail "$bond: $(vm "cat /proc/net/bonding/$bond")"
vm "grep -q 'MII Status: up' /proc/net/bonding/$bond" || fail "$bond's links are down: $(vm "cat /proc/net/bonding/$bond")"
[[ $(mac_of "$bond") == "$spare1_mac" ]] || fail "$bond has $(mac_of "$bond"), not $spare1's $spare1_mac"
check "$bond: active-backup with $spare1 and $spare2, up, with $spare1's MAC address"
change PUT "/network/interfaces/$bond" '{"ipv4":{"mode":"dhcp"},"ipv6":"off","dns":[]}'
wait_for "$bond's address from the spare network's DHCP" "[[ \$(address_of $bond) == 10.0.3.* ]]" 60
confirm
check "$bond has $(address_of "$bond") from its own network's DHCP"
change POST /network/devices "$(jq -cn --arg v "$vlan" --arg b "$bond" --arg a "$vlan_address" \
    '{kind: "vlan", name: $v, parent: $b, id: 42, settings: {ipv4: {mode: "static", address: $a, prefix: 24, gateway: null}, ipv6: "off"}}')"
wait_for "$vlan" "link_exists $vlan" 30
confirm
vm "ip -d link show dev $vlan | grep -q 'vlan protocol 802.1Q id 42'" || fail "$vlan: $(vm "ip -d link show dev $vlan")"
wait_for "$vlan up with $vlan_address" "[[ \$(address_of $vlan) == $vlan_address ]] && vm 'ip link show dev $vlan | grep -q \",UP\"'" 30
[[ -n $(address_of "$bond") ]] || fail "$bond lost its address carrying the VLAN"
check "$vlan is up on $bond with id 42 and $vlan_address; $bond keeps its address"
call GET /network/setup | jq -e --arg v "$vlan" --arg b "$bond" '.devices[] | select(.name == $v) | .parent == $b and .vlan_id == 42' \
    >/dev/null || fail "the API: $(call GET /network/setup | jq -c .devices)"
[[ $(status DELETE "/network/devices/$bond") == 422 ]] || fail "$bond went with a VLAN on it: $(cat "$work/last")"
check "$bond stays while it carries $vlan"
change DELETE "/network/devices/$vlan"
wait_for "$vlan to go" "! link_exists $vlan" 30
confirm
change DELETE "/network/devices/$bond"
wait_for "$bond to go" "! link_exists $bond" 30
confirm
[[ -z $(master_of "$spare1") && -z $(master_of "$spare2") ]] || fail "a spare card is still in a bond"
check "$vlan and $bond removed, their links gone"

say "5. The bridge removed: $card gets its settings back"
plan=$(call POST /network/devices/plan "$(jq -cn --arg b "$bridge" '{action: "remove", name: $b}')")
jq -e --arg c "$card" --arg a "$address" \
    '.interfaces[] | select(.interface == $c) | .becomes.settings.ipv4.keep == $a' <<<"$plan" >/dev/null \
    || fail "the plan: $plan"
change DELETE "/network/devices/$bridge"
wait_for "$card to have $address again" "[[ \$(address_of $card) == $address ]]" 60
confirm
link_exists "$bridge" && fail "$bridge's link stayed"
[[ -z $(master_of "$card") ]] || fail "$card is still a port"
check "$card has $address again, $bridge is gone"
wait_for "zendiloOS's chain to go" "! vm 'sudo iptables -S ZENDILOOS-BRIDGES' && ! vm 'sudo iptables -S DOCKER-USER | grep -q ZENDILOOS'" 30
check "zendiloOS's chain and its jump are gone from Docker's tables"

say "6. The firewall's rules and Docker's tables as they were"
# The firewall refreshes what it knows of the bridges every 30 seconds.
wait_for "the firewall's table as before" "[[ \"\$(firewall_table)\" == \"\$firewall_before\" ]]" 90
check "the firewall's table is as it was before the bridges"
diff <(echo "$docker_before") <(docker_tables) || fail "Docker's tables changed"
check "Docker's tables are as Docker made them"
[[ $(call GET /network/setup | jq -c '[.pending, (.devices | length)]') == '[null,0]' ]] || fail "something is left"
for spare in "$spare1" "$spare2"; do
    # Switched off when the bond went; as before the test.
    vm "sudo rm -f /etc/systemd/network/05-zendiloos-$spare.network"
done
vm 'sudo networkctl reload'

say "Passed: bridges, bonds and VLANs made, kept once confirmed, gone back when not, frames bridged past Docker"
