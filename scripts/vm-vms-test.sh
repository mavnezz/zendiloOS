#!/usr/bin/env bash
# Virtual machines end to end in the second test VM (nested KVM), through
# the HTTPS API, with virsh, zfs and nft inside the VM to look:
#
#   1. the plugin's hardware check sees KVM; libvirt and QEMU are installed
#      through the API as a job (apt in a unit of its own), without
#      libvirt's network, its filters or dnsmasq
#   2. the VMs' storage goes on an existing pool: zendiloos.vms mounted at
#      /volume<N>/@vms, libvirt waiting for it
#   3. a VM with 1 vCPU, 256 MiB, UEFI, a 1 GiB disk and no ISO is made in
#      a job: a sparse zvol with volmode=dev, a domain zendiloos-<id> with
#      passt; it starts (into its firmware) and runs as libvirt-qemu
#   4. its console's WebSocket answers with QEMU's VNC greeting
#      (RFB 003.00x); a console from another origin is refused
#   5. it shuts down (ACPI, or forced off when the firmware does not
#      listen), is deleted only with its name typed again, and its zvol,
#      domain and XML are gone
#   6. optionally, with ZENDILOOS_VMS_ISO=<share id>:<path in the share>
#      naming an .iso the account may read: a VM from it, the ISO copied
#      to /volume<N>/@vms/isos and removed with the VM
#   7. no network, bridge, dnsmasq or firewall table of libvirt's came, and
#      nftables holds zendiloOS's rules as before
#
# Needs the VM running with the package installed and set up, a pool
# zendiloOS made (ZENDILOOS_VMS_POOL picks it, else the one Docker's data
# is on, else the first), the network for apt and node on this machine;
# account and password as for vm-storage-test.sh. Runs in the second VM by
# default: it installs packages and starts a VM there. The plugin stays
# installed and its storage set up; the test VM goes.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
name="ztest vm"
iso_name="ztest iso"
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/admin.jar" -c "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

status() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -o "$work/last" -w '%{http_code}' -b "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf")")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-120}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The state of job $1 once it ended: "succeeded …" or "failed <code> <detail>".
job_end() {
    local id=$1 seconds=${2:-600}
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")'
}

job_succeeded() {
    local state
    state=$(job_end "$@")
    [[ $state == succeeded* ]] || fail "job $1: $state"
}

# The id of the VM called $1, if there is one.
vm_id() { call GET /vms | jq -r --arg n "$1" '.[] | select(.name == $n) | .id'; }

vm_state() { call GET "/vms/$1" | jq -r .state; }

# Deletes the VM called $1 through the API, powered off first if it runs.
delete_vm() {
    local id
    id=$(vm_id "$1")
    [[ -n $id ]] || return 0
    if [[ $(vm_state "$id") == running ]]; then call POST "/vms/$id/force-off" >/dev/null; fi
    call POST "/vms/$id/delete" "$(jq -n --arg n "$1" '{confirm: $n}')" >/dev/null
}

cleanup() {
    set +e
    if [[ -s $work/admin.csrf ]]; then
        delete_vm "$name"
        delete_vm "$iso_name"
    fi
    rm -rf "$work"
}
trap cleanup EXIT

# Everything of libvirt's that would mean its own network: tables or chains
# in nftables, a bridge, dnsmasq, a defined network.
libvirt_networking() {
    vm "sudo nft list ruleset 2>/dev/null | grep -i -E 'libvirt|virbr' || true
        ip -o link show 2>/dev/null | grep -E ': virbr' || true
        pgrep -a dnsmasq | grep -i libvirt || true
        sudo virsh -c qemu:///system net-list --all --name 2>/dev/null | grep -v '^\$' || true"
}

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
session=$(awk '$6 ~ /zendiloos_session$/ { print $6 "=" $7 }' "$work/admin.jar")
command -v node >/dev/null || fail "node is needed on this machine for the console's probe"

# What nftables holds before: the test checks nothing of libvirt's comes.
vm 'sudo nft list tables' | sort >"$work/tables.before"
firewall_before=$(vm 'sudo nft list table inet zendiloos_firewall 2>/dev/null || true')
[[ -n $(libvirt_networking) ]] && fail "libvirt's networking is there before the test: $(libvirt_networking)"

say "Cleaning up an earlier run"
delete_vm "$name"
delete_vm "$iso_name"

say "1. The hardware check sees KVM; libvirt and QEMU are installed as a job"
plugin=$(call GET /vms/plugin)
jq -e '.hardware.kvm == true' <<<"$plugin" >/dev/null || fail "no KVM in the VM (nested KVM off?): $plugin"
note "CPU: $(jq -r '.hardware.cpu_virtualization // "no flag"' <<<"$plugin"), IOMMU: $(jq -r .hardware.iommu <<<"$plugin"), $(jq -r .cores <<<"$plugin") threads"
if jq -e '.installed == false' <<<"$plugin" >/dev/null; then
    [[ $(status POST /vms/plugin/install) == 202 ]] || fail "install: $(cat "$work/last")"
    job=$(jq -r .id "$work/last")
    job_succeeded "$job" 1800
    record=$(vm 'cat /var/lib/zendiloos-packages/run.json') || fail "the run was not recorded"
    jq -e '.state == "succeeded" and .task == "install_vms"' <<<"$record" >/dev/null || fail "the record: $record"
    note "installed in job $job"
else
    note "installed already"
fi
[[ $(status POST /vms/plugin/install) == 409 ]] && jq -e '.code == "vms_installed"' "$work/last" >/dev/null \
    || fail "a second install was not refused: $(cat "$work/last")"
for package in libvirt-daemon libvirt-daemon-driver-qemu libvirt-clients qemu-system-x86 qemu-utils ovmf passt; do
    vm "dpkg-query -W -f='\${Status}' $package 2>/dev/null | grep -q 'install ok installed'" || fail "$package is not installed"
done
for package in libvirt-daemon-system libvirt-daemon-config-network libvirt-daemon-config-nwfilter \
    libvirt-daemon-driver-network libvirt-daemon-driver-nwfilter dnsmasq-base; do
    if vm "dpkg-query -W -f='\${Status}' $package 2>/dev/null | grep -q 'install ok installed'"; then
        fail "$package came along"
    fi
done

say "2. The VMs' storage goes on a pool"
plugin=$(call GET /vms/plugin)
pool=$(jq -r '.storage.pool // empty' <<<"$plugin")
if [[ -z $pool ]]; then
    pool=${ZENDILOOS_VMS_POOL:-$(call GET /apps/docker | jq -r '.volume.pool // empty')}
    pool=${pool:-$(jq -r '.pools[0].pool // empty' <<<"$plugin")}
    [[ -n $pool ]] || fail "no pool to put the VMs' storage on: create one first"
    [[ $(status POST /vms/plugin/storage "{\"pool\":\"$pool\"}") == 200 ]] || fail "storage: $(cat "$work/last")"
fi
plugin=$(call GET /vms/plugin)
volume=$(jq -er '.storage.volume' <<<"$plugin") || fail "the storage is not set up: $plugin"
note "on $pool (volume $volume)"
vm "findmnt -n -o SOURCE /volume$volume/@vms" | grep -qx "$pool/zendiloos.vms" || fail "/volume$volume/@vms is not mounted"
vm "sudo zfs get -H -o value setuid,devices,exec $pool/zendiloos.vms" | tr '\n' ' ' | grep -q '^off off off' \
    || fail "the dataset allows setuid, devices or exec"
vm 'sudo cat /etc/systemd/system/libvirtd.service.d/zendiloos.conf' | grep -q "RequiresMountsFor=/volume$volume/@vms" \
    || fail "libvirt does not wait for the storage"
vm 'systemctl is-active --quiet libvirtd.service' || fail "libvirtd does not run"
vm 'test -d /run/zendiloos-vms' || fail "the consoles' folder is missing"
[[ $(vm 'stat -c %U:%G:%a /run/zendiloos-vms') == libvirt-qemu:root:2770 ]] \
    || fail "the consoles' folder is $(vm 'stat -c %U:%G:%a /run/zendiloos-vms')"

say "3. A VM with 1 vCPU, 256 MiB, UEFI and a 1 GiB disk is made and starts"
new=$(jq -n --arg n "$name" '{name: $n, os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi", disk_gib: 1,
    network: {kind: "user"}, autostart: false}')
[[ $(status POST /vms "$(jq '.cpus = 999' <<<"$new")") == 422 ]] || fail "more CPUs than the machine has: $(cat "$work/last")"
[[ $(status POST /vms "$new") == 202 ]] || fail "create: $(cat "$work/last")"
id=$(jq -er .vm.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 300
domain="zendiloos-$id"
zvol="$pool/zendiloos.vms/$id-disk0"
note "$domain on $zvol"
[[ $(vm_state "$id") == shut_off ]] || fail "after creating: $(call GET "/vms/$id")"
[[ $(status POST /vms "$new") == 422 ]] || fail "a second VM of the same name: $(cat "$work/last")"
read -r volsize volmode refreservation < <(vm "sudo zfs get -H -p -o value volsize,volmode,refreservation $zvol" | tr '\n' ' '; echo)
[[ $volsize == $((1 << 30)) && $volmode == dev && $refreservation == 0 ]] \
    || fail "the zvol: volsize $volsize, volmode $volmode, refreservation $refreservation"
xml=$(vm "sudo virsh -c qemu:///system dumpxml $domain")
grep -q "<backend type='passt'/>" <<<"$xml" || fail "no passt: $xml"
grep -q "socket='/run/zendiloos-vms/$id.vnc'" <<<"$xml" || fail "no console socket: $xml"
grep -q "firmware='efi'" <<<"$xml" || fail "not UEFI: $xml"
vm "sudo test -f /etc/zendiloos/vms/$domain.xml" || fail "the domain's XML is not kept"
if [[ $(status POST "/vms/$id/start") != 200 ]]; then
    # What QEMU or AppArmor said, for a start that failed.
    note "libvirt: $(vm "sudo tail -n 5 /var/log/libvirt/qemu/$domain.log" 2>&1)"
    note "AppArmor: $(vm "sudo journalctl -k -n 200 --no-pager | grep -i 'apparmor=\"DENIED\"' | tail -n 5" 2>&1)"
    fail "start: $(cat "$work/last")"
fi
jq -e '.state == "running"' "$work/last" >/dev/null || fail "not running: $(cat "$work/last")"
[[ $(vm "sudo virsh -c qemu:///system domstate $domain") == running ]] || fail "libvirt does not say it runs"
[[ $(status POST "/vms/$id/start") == 409 ]] || fail "a second start: $(cat "$work/last")"
vm "pgrep -u libvirt-qemu -f 'guest=$domain' >/dev/null" || fail "QEMU does not run as libvirt-qemu"
vm "pgrep -u libvirt-qemu passt >/dev/null" || note "passt does not show as a process of libvirt-qemu (named otherwise?)"

say "4. Its console answers with QEMU's VNC greeting"
wss="wss://${base#https://}/api/v1/vms/$id/console"
vm "sudo test -S /run/zendiloos-vms/$id.vnc" || fail "no console socket in the VM"
greeting=""
for attempt in 1 2 3; do
    if greeting=$(NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-vnc-probe.mjs" "$wss" "$session" "$base" 2>&1); then
        break
    fi
    note "attempt $attempt: $greeting"
    sleep 3
done
[[ $greeting =~ RFB\ 003\.00[0-9] ]] || fail "the console: $greeting"
note "$greeting"
if NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-vnc-probe.mjs" "$wss" "$session" "https://evil.example" >/dev/null 2>&1; then
    fail "a console was opened from another origin"
fi
call GET /logs/audit | jq -e '[.. | strings | select(test("vms.console"))] | length > 0' >/dev/null \
    || fail "no audit entry for the console"

say "5. It shuts down, is deleted with its name typed again, and its disk goes"
[[ $(status POST "/vms/$id/shutdown") == 200 ]] || fail "shutdown: $(cat "$work/last")"
for _ in $(seq 1 20); do
    [[ $(vm_state "$id") == shut_off ]] && break
    sleep 1
done
if [[ $(vm_state "$id") != shut_off ]]; then
    note "the firmware did not take the ACPI shutdown; forced off"
    [[ $(status POST "/vms/$id/force-off") == 200 ]] || fail "force off: $(cat "$work/last")"
fi
wait_for "the VM to be off" "[[ \$(vm_state $id) == shut_off ]]" 30
[[ $(status POST "/vms/$id/delete" '{"confirm":"ztest"}') == 422 ]] || fail "deleted with the wrong name: $(cat "$work/last")"
[[ $(status POST "/vms/$id/delete" "$(jq -n --arg n "$name" '{confirm: $n}')") == 204 ]] || fail "delete: $(cat "$work/last")"
[[ $(status GET "/vms/$id") == 404 ]] || fail "still listed: $(cat "$work/last")"
if vm "sudo zfs list -H -o name $zvol" >/dev/null 2>&1; then fail "the zvol $zvol is still there"; fi
if vm "sudo virsh -c qemu:///system domstate $domain" >/dev/null 2>&1; then fail "the domain $domain is still defined"; fi
vm "sudo test ! -e /etc/zendiloos/vms/$domain.xml" || fail "the domain's XML stayed"
for action in vms.create vms.start vms.shutdown vms.delete; do
    call GET /logs/audit | jq -e --arg a "$action" '[.. | strings | select(. == $a)] | length > 0' >/dev/null \
        || fail "no audit entry $action"
done

if [[ -n ${ZENDILOOS_VMS_ISO:-} ]]; then
    say "6. A VM from an ISO in a share: copied beside the VMs, removed with it"
    share_id=${ZENDILOOS_VMS_ISO%%:*}
    path=${ZENDILOOS_VMS_ISO#*:}
    body=$(jq -n --arg n "$iso_name" --argjson s "$share_id" --arg p "$path" '{name: $n, os: "linux", cpus: 1,
        memory_mib: 256, firmware: "bios", disk_gib: 1, network: {kind: "user"}, iso: {share_id: $s, path: $p}}')
    [[ $(status POST /vms "$body") == 202 ]] || fail "create from the ISO: $(cat "$work/last")"
    iso_id=$(jq -er .vm.id "$work/last")
    job_succeeded "$(jq -r .job.id "$work/last")" 1800
    copy="/volume$volume/@vms/isos/$iso_id.iso"
    vm "sudo test -f $copy" || fail "no copy at $copy"
    vm "sudo virsh -c qemu:///system dumpxml zendiloos-$iso_id" | grep -q "<source file='$copy'/>" || fail "the copy is not attached"
    [[ $(status POST "/vms/$iso_id/delete" "$(jq -n --arg n "$iso_name" '{confirm: $n}')") == 204 ]] \
        || fail "delete: $(cat "$work/last")"
    vm "sudo test ! -e $copy" || fail "the copy stayed"
else
    say "6. Skipped: set ZENDILOOS_VMS_ISO=<share id>:<path> to make a VM from an ISO"
fi

say "7. Nothing of libvirt's networking came, and zendiloOS's rules are as they were"
found=$(libvirt_networking)
[[ -z $found ]] || fail "libvirt's networking appeared: $found"
vm 'sudo nft list tables' | sort >"$work/tables.after"
diff -u "$work/tables.before" "$work/tables.after" || fail "nftables' tables changed"
firewall_after=$(vm 'sudo nft list table inet zendiloos_firewall 2>/dev/null || true')
if [[ -n $firewall_before ]]; then
    [[ $firewall_after == "$firewall_before" ]] || fail "zendiloOS's firewall rules changed"
    note "zendiloOS's firewall table is in force as before"
else
    [[ -z $firewall_after ]] || fail "a firewall table appeared"
    note "the firewall is off; no table came"
fi

say "All virtual machine tests passed"
