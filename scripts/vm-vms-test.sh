#!/usr/bin/env bash
# Virtual machines end to end in the second test VM (nested KVM), through
# the HTTPS API, with virsh, zfs and nft inside the VM to look:
#
#   1. the plugin's hardware check sees KVM; libvirt and QEMU are installed
#      through the API as a job (apt in a unit of its own), without
#      libvirt's network, its filters or dnsmasq
#   2. the VMs' storage goes on an existing pool: zendiloos.vms mounted at
#      /volume<N>/@vms, libvirt waiting for it
#   3. a VM with 1 vCPU, 256 MiB, UEFI, a 1 GiB disk and no image is made
#      in a job: a sparse zvol with volmode=dev, a domain zendiloos-<id>
#      with passt; it starts (into its firmware) and runs as libvirt-qemu
#   4. its console's WebSocket answers with QEMU's VNC greeting
#      (RFB 003.00x); a console from another origin is refused
#   5. it shuts down (ACPI, or forced off when the firmware does not
#      listen), is deleted only with its name typed again, and its zvol,
#      domain and XML are gone
#   6. an ISO (Alpine's virt ISO, downloaded on this machine and kept in
#      ~/.cache/zendiloos, or ZENDILOOS_VMS_ISO_FILE, else a dummy) is put
#      into a share of the test's and added to the library from the NAS in
#      a job: the very same bytes in /volume<N>/@vms/isos/<id>.iso, no unit
#      left; the same name twice is refused
#   7. the ISO is uploaded through the API as the browser sends it: the
#      same bytes again, nothing beside them, no unit left; an upload cut
#      off halfway leaves neither a record nor a file
#   8. a VM with 2 disks, 2 network cards (the second on a bridge if the
#      NAS has one) and the ISO in its drive is made: both sparse zvols,
#      both cards with the MAC addresses the API names, the ISO attached
#      and booted first
#   9. while it is off, its vCPUs and memory change and a disk grows; it
#      starts (from the ISO), and then a hardware change is refused while
#      its name changes
#  10. the ISO is ejected and inserted again while it runs, at once in the
#      running guest and in its definition
#  11. the live WebSocket's topic vm_stats brings its load
#  12. the ISO is not deleted while the VM has it; the VM goes with its
#      disks, then both images with their files
#  13. no network, bridge, dnsmasq or firewall table of libvirt's came, and
#      nftables holds zendiloOS's rules as before
#  14. a second storage location goes on another pool (ZENDILOOS_VMS_POOL2,
#      else the first pool without one): its dataset mounted at
#      /volume<N>/@vms, libvirt waiting for both; neither pool can be
#      deleted, and the location is listed
#  15. a VM with a disk on each location, a card on an internal network
#      (a bridge without members made through the Network API and
#      confirmed) and one directly on the NAS's card: both zvols on their
#      pools, the domain's XML with a bridge and a macvtap card, the NAS
#      without an address on the internal bridge and kept off it by
#      nftables' bridge table; started, the VM's tap is in the bridge and
#      a macvtap sits on the card; the location in use cannot be removed,
#      nor can the card be taken into a bridge
#  16. the log shows the test's VM events (searched by its name)
#  17. the VM goes, then the internal network (its bridge table with it)
#      and the location the test added, with the pool's name typed again
#
# Needs the VM running with the package installed and set up, a pool
# zendiloOS made (ZENDILOOS_VMS_POOL picks it, else the one Docker's data
# is on, else the first), the network for apt, node and sha256sum on this
# machine (and the network here once, for Alpine's ISO); account and
# password as for vm-storage-test.sh, and from step 14 on a second pool
# without the VMs' storage and the network managed by systemd-networkd
# (as `just vm-network-test` leaves it). Runs in the second VM by default
# (2 cores, 2 GB, pools apps and tank): it installs packages and starts
# VMs there. The plugin stays installed and its first location set up;
# the test's VMs, images, share, internal network and the location it
# added go, also when it fails.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
name="ztest vm"
big="ztest big"
share="ztestvms"
image_name="ztest alpine"
upload_name="ztest upload"
cut_name="ztest cut"
places="ztest places"
internal=ztvmnet0
# Set once the test made them, so that cleaning up removes only its own.
internal_made=""
location_added=""
cache=${XDG_CACHE_HOME:-$HOME/.cache}/zendiloos
alpine="https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64"
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

# The status code of $3 sent as it is to the API path $1 with the query $2,
# as a browser's upload: streamed, of the length the file has. More curl
# arguments may follow.
send_file() {
    local path=$1 query=$2 file=$3
    shift 3
    curl -sk -o "$work/last" -w '%{http_code}' -b "$work/admin.jar" -X POST -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/admin.csrf")" -H 'Content-Type: application/octet-stream' \
        -T "$file" "$@" "$api$path?$query"
}

urlencode() { jq -rn --arg v "$1" '$v | @uri'; }

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

# Fails unless the VM's answer in $work/last has the cards of $2 (a GET
# from before), slot by slot. Should it not, it says what a GET now,
# libvirt and zendiloosd's journal (which logs the cards of each VM recorded
# and changed, issue #71) tell, so the next time shows where the addresses
# came from.
same_cards() {
    local id=$1 before=$2 after
    after=$(jq -c '[.nics[] | [.slot, .mac]]' "$work/last")
    [[ $after == "$(jq -c '[.nics[] | [.slot, .mac]]' <<<"$before")" ]] && return 0
    note "the answer: $after"
    note "before: $(jq -c '[.nics[] | [.slot, .mac]]' <<<"$before")"
    note "GET now: $(call GET "/vms/$id" | jq -c '[.nics[] | [.slot, .mac]]')"
    note "libvirt: $(vm "sudo virsh -c qemu:///system dumpxml --inactive zendiloos-$id" | grep -o "mac address='[^']*'" | tr '\n' ' ')"
    note "journal: $(vm "sudo journalctl -u zendiloosd --since '-15 min' --no-pager -o cat" | grep "$id" | grep -E 'vms: (recorded|changed)' | tail -n 5)"
    fail "the cards' addresses changed"
}

# The id of the VM called $1, if there is one.
vm_id() { call GET /vms | jq -r --arg n "$1" '.[] | select(.name == $n) | .id'; }

vm_state() { call GET "/vms/$1" | jq -r .state; }

# Deletes the VM called $1 through the API, powered off first if it runs.
delete_vm() {
    local id
    id=$(vm_id "$1")
    [[ -n $id ]] || return 0
    if [[ $(vm_state "$id") != shut_off ]]; then call POST "/vms/$id/force-off" >/dev/null; fi
    call POST "/vms/$id/delete" "$(jq -n --arg n "$1" '{confirm: $n}')" >/dev/null
}

# The id of the image called $1, if there is one.
image_id() { call GET /vms/images | jq -r --arg n "$1" '.[] | select(.name == $n) | .id'; }

delete_image() {
    local id
    id=$(image_id "$1")
    [[ -n $id ]] && call DELETE "/vms/images/$id" >/dev/null
    return 0
}

delete_share() {
    local id
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
    done
}

# The change of the network that waits for confirmation, `null` for none.
pending() { call GET /network/setup | jq -c .pending 2>/dev/null || true; }

# Sends a change of the network (a bridge made or removed) and confirms
# it once it waits; the status code of the change, 000 if none came.
network_change() {
    local method=$1 path=$2 body=${3:-} code
    code=$(status "$method" "$path" "$body")
    [[ $code == 202 ]] || { printf '%s' "$code"; return 0; }
    for _ in $(seq 1 60); do
        [[ $(pending) == '{'* ]] && break
        sleep 1
    done
    [[ $(status POST /network/confirm) == 204 ]] || { printf 'confirm'; return 0; }
    printf '%s' "$code"
}

# Removes the internal network the test made, once no VM is on it.
remove_internal() {
    call GET /network/setup | jq -e --arg n "$internal" '.devices[] | select(.name == $n)' >/dev/null 2>&1 || return 0
    # A change of the test's that waits (cut off before it was confirmed) goes first.
    if [[ $(pending) == '{'* ]]; then status POST /network/confirm >/dev/null; fi
    network_change DELETE "/network/devices/$internal" >/dev/null
}

clean() {
    delete_vm "$name"
    delete_vm "$big"
    delete_vm "renamed vm"
    delete_vm "$places"
    delete_image "$image_name"
    delete_image "$upload_name"
    delete_image "$cut_name"
    delete_share
    [[ -n $internal_made ]] && remove_internal
    if [[ -n $location_added ]]; then
        status POST "/vms/locations/$location_added/delete" "$(jq -n --arg p "$location_added" '{confirm: $p}')" >/dev/null
    fi
    return 0
}

cleanup() {
    set +e
    [[ -s $work/admin.csrf ]] && clean
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

# An ISO on this machine: ZENDILOOS_VMS_ISO_FILE, else Alpine's virt ISO,
# downloaded once and checked against its checksum, else a dummy one that
# boots nothing (the library, media and load checks hold all the same).
the_iso() {
    if [[ -n ${ZENDILOOS_VMS_ISO_FILE:-} ]]; then
        [[ -s $ZENDILOOS_VMS_ISO_FILE ]] || fail "no ISO at $ZENDILOOS_VMS_ISO_FILE"
        printf '%s' "$ZENDILOOS_VMS_ISO_FILE"
        return
    fi
    mkdir -p "$cache"
    local file
    file=$(curl -fsSL --max-time 30 "$alpine/latest-releases.yaml" 2>/dev/null \
        | sed -n 's/^ *file: \(alpine-virt-[0-9.]*-x86_64\.iso\)$/\1/p' | head -1) || true
    if [[ -n $file && ! -s $cache/$file ]]; then
        note "downloading $file" >&2
        if curl -fsSL --max-time 900 -o "$cache/$file.part" "$alpine/$file" \
            && curl -fsSL --max-time 30 -o "$cache/$file.sha256" "$alpine/$file.sha256" \
            && (cd "$cache" && sed "s/$file\$/$file.part/" "$file.sha256" | sha256sum -c --quiet -); then
            mv "$cache/$file.part" "$cache/$file"
        else
            rm -f "$cache/$file.part"
        fi
    fi
    if [[ -z $file ]]; then
        file=$(cd "$cache" && ls -t alpine-virt-*-x86_64.iso 2>/dev/null | head -1) || true
    fi
    if [[ -n $file && -s $cache/$file ]]; then
        printf '%s' "$cache/$file"
        return
    fi
    note "no Alpine ISO (no network?): a dummy ISO of 8 MiB, which boots nothing" >&2
    head -c $((8 << 20)) /dev/urandom >"$work/ztest-dummy.iso"
    printf '%s' "$work/ztest-dummy.iso"
}

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
session=$(awk '$6 ~ /zendiloos_session$/ { print $6 "=" $7 }' "$work/admin.jar")
command -v node >/dev/null || fail "node is needed on this machine for the WebSocket probes"
command -v sha256sum >/dev/null || fail "sha256sum is needed on this machine"
me=$(call GET /auth/session | jq -er .account.id) || fail "no account"

# What nftables holds before: the test checks nothing of libvirt's comes.
vm 'sudo nft list tables' | sort >"$work/tables.before"
firewall_before=$(vm 'sudo nft list table inet zendiloos_firewall 2>/dev/null || true')
[[ -n $(libvirt_networking) ]] && fail "libvirt's networking is there before the test: $(libvirt_networking)"

say "Cleaning up an earlier run"
clean

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
isos="/volume$volume/@vms/isos"
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
new=$(jq -n --arg n "$name" '{name: $n, os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi",
    disks: [{slot: 0, size_gib: 1, bus: "virtio"}],
    nics: [{slot: 0, model: "virtio", network: {kind: "user"}}],
    drives: [], boot: [{kind: "disk", slot: 0}], autostart: false}')
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
    if greeting=$(NODE_NO_WARNINGS=1 NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-vnc-probe.mjs" "$wss" "$session" "$base" 2>&1); then
        break
    fi
    note "attempt $attempt: $greeting"
    sleep 3
done
[[ $greeting =~ RFB\ 003\.00[0-9] ]] || fail "the console: $greeting"
note "$greeting"
if NODE_NO_WARNINGS=1 NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-vnc-probe.mjs" "$wss" "$session" "https://evil.example" >/dev/null 2>&1; then
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

say "6. An ISO from a share goes into the library, copied in a job"
iso=$(the_iso)
iso_bytes=$(stat -c %s "$iso")
iso_sum=$(sha256sum "$iso" | cut -d' ' -f1)
note "$(basename "$iso"), $iso_bytes bytes"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name: $n, pool: $pool, access: [{kind: "account", id: $me, access: "read_write"}]}')" | jq -er .id) \
    || fail "the test's share"
[[ $(send_file "/files/shares/$share_id/upload" "path=&name=alpine.iso" "$iso") == 201 ]] \
    || fail "the ISO into the share: $(cat "$work/last")"
add=$(jq -n --argjson s "$share_id" --arg n "$image_name" '{share_id: $s, path: "alpine.iso", name: $n}')
[[ $(status POST /vms/images "$add") == 202 ]] || fail "add from the NAS: $(cat "$work/last")"
image=$(jq -er .image.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 900
listed=$(call GET /vms/images | jq --arg id "$image" '.[] | select(.id == $id)')
jq -e --argjson b "$iso_bytes" '.ready and .bytes == $b and .origin.kind == "share" and .origin.path == "alpine.iso"' \
    <<<"$listed" >/dev/null || fail "the image: $listed"
[[ $(vm "sudo sha256sum $isos/$image.iso" | cut -d' ' -f1) == "$iso_sum" ]] || fail "the copy differs from the ISO"
[[ $(vm "sudo stat -c %U:%a $isos/$image.iso") == root:644 ]] || fail "the copy is $(vm "sudo stat -c %U:%a $isos/$image.iso")"
if vm "systemctl is-active --quiet zendiloos-vm-iso-$image.service"; then fail "the copy's unit is left"; fi
[[ $(status POST /vms/images "$add") == 409 ]] && jq -e '.code == "name_taken"' "$work/last" >/dev/null \
    || fail "the same name twice: $(cat "$work/last")"
note "image $image"

say "7. The ISO is uploaded as the browser sends it, and an upload cut off leaves nothing"
[[ $(send_file /vms/images/upload "name=$(urlencode "$upload_name")" "$iso") == 201 ]] \
    || fail "upload: $(cat "$work/last")"
uploaded=$(jq -er .id "$work/last")
jq -e --argjson b "$iso_bytes" --arg n "$upload_name" '.ready and .bytes == $b and .name == $n and .origin.kind == "upload"' \
    "$work/last" >/dev/null || fail "the upload: $(cat "$work/last")"
[[ $(vm "sudo sha256sum $isos/$uploaded.iso" | cut -d' ' -f1) == "$iso_sum" ]] || fail "the upload differs from the ISO"
[[ $(vm "sudo stat -c %U:%a $isos/$uploaded.iso") == root:644 ]] || fail "the upload is $(vm "sudo stat -c %U:%a $isos/$uploaded.iso")"
if vm "systemctl is-active --quiet zendiloos-vm-upload-$uploaded.service"; then fail "the upload's unit is left"; fi
files_before=$(vm "sudo ls -A $isos" | sort)
# At 64 KiB/s for 3 s: cut off long before the end.
code=$(send_file /vms/images/upload "name=$(urlencode "$cut_name")" "$iso" --limit-rate 64K --max-time 3 || true)
note "the cut-off upload's request ended with ${code:-nothing}"
wait_for "the cut-off upload to go" "[[ -z \$(image_id '$cut_name') ]]" 60
wait_for "no upload unit to run" "! vm 'systemctl list-units --no-legend --state=active \"zendiloos-vm-upload-*\"' | grep -q ."  60
[[ $(vm "sudo ls -A $isos" | sort) == "$files_before" ]] || fail "the cut-off upload left: $(vm "sudo ls -A $isos")"
note "image $uploaded"

say "8. A VM with 2 disks, 2 network cards and the ISO in its drive"
bridge=$(jq -r '.bridges[0] // empty' <<<"$plugin")
second='{"kind":"user"}'
[[ -n $bridge ]] && second=$(jq -n --arg b "$bridge" '{kind: "bridge", bridge: $b}')
new=$(jq -n --arg n "$big" --arg img "$image" --argjson second "$second" '{name: $n, os: "linux", cpus: 1,
    memory_mib: 384, firmware: "bios",
    disks: [{slot: 0, size_gib: 1, bus: "virtio"}, {slot: 1, size_gib: 1, bus: "sata"}],
    nics: [{slot: 0, model: "virtio", network: {kind: "user"}}, {slot: 1, model: "e1000e", network: $second}],
    drives: [{slot: 0, image: $img}],
    boot: [{kind: "drive", slot: 0}, {kind: "disk", slot: 0}], autostart: false}')
[[ $(status POST /vms "$new") == 202 ]] || fail "create: $(cat "$work/last")"
big_id=$(jq -er .vm.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 300
domain="zendiloos-$big_id"
note "$domain, the second card ${bridge:+on $bridge}${bridge:-through passt}"
for disk in 0 1; do
    read -r volsize refreservation < <(vm "sudo zfs get -H -p -o value volsize,refreservation $pool/zendiloos.vms/$big_id-disk$disk" | tr '\n' ' '; echo)
    [[ $volsize == $((1 << 30)) && $refreservation == 0 ]] || fail "disk $disk: volsize $volsize, refreservation $refreservation"
done
vmjson=$(call GET "/vms/$big_id")
xml=$(vm "sudo virsh -c qemu:///system dumpxml --inactive $domain")
for mac in $(jq -r '.nics[].mac' <<<"$vmjson"); do
    grep -q "<mac address='$mac'/>" <<<"$xml" || fail "no card with $mac: $xml"
done
[[ $(grep -c "<interface " <<<"$xml") == 2 ]] || fail "not two cards: $xml"
grep -q "<source file='$isos/$image.iso'/>" <<<"$xml" || fail "the ISO is not in the drive: $xml"
grep -A4 "$isos/$image.iso" <<<"$xml" | grep -q "<boot order='1'/>" || fail "the drive is not booted first: $xml"
grep -q "$big_id-disk1'/>" <<<"$xml" || fail "no second disk: $xml"

say "9. Off, its hardware changes; running, it is refused while its name changes"
change=$(jq '{name, cpus: 2, memory_mib: 512, firmware, disks: (.disks | map(if .slot == 1 then .size_gib = 2 else . end)),
    nics: [.nics[] | {slot, model, network}], drives, boot, autostart}' <<<"$vmjson")
[[ $(status PUT "/vms/$big_id" "$change") == 200 ]] || fail "change while off: $(cat "$work/last")"
jq -e '.cpus == 2 and .memory_mib == 512' "$work/last" >/dev/null || fail "not changed: $(cat "$work/last")"
same_cards "$big_id" "$vmjson"
xml=$(vm "sudo virsh -c qemu:///system dumpxml --inactive $domain")
grep -q "<vcpu placement='static'>2</vcpu>" <<<"$xml" || fail "libvirt has not 2 vCPUs: $xml"
grep -q "<memory unit='KiB'>524288</memory>" <<<"$xml" || fail "libvirt has not 512 MiB: $xml"
[[ $(vm "sudo zfs get -H -p -o value volsize $pool/zendiloos.vms/$big_id-disk1") == $((2 << 30)) ]] || fail "disk 1 did not grow"
shrink=$(jq '.disks |= map(if .slot == 1 then .size_gib = 1 else . end)' <<<"$change")
[[ $(status PUT "/vms/$big_id" "$shrink") == 422 ]] || fail "a disk shrank: $(cat "$work/last")"
if [[ $(status POST "/vms/$big_id/start") != 200 ]]; then
    note "libvirt: $(vm "sudo tail -n 5 /var/log/libvirt/qemu/$domain.log" 2>&1)"
    note "AppArmor: $(vm "sudo journalctl -k -n 200 --no-pager | grep -i 'apparmor=\"DENIED\"' | tail -n 5" 2>&1)"
    fail "start: $(cat "$work/last")"
fi
running=$(jq '.cpus = 1' <<<"$change")
[[ $(status PUT "/vms/$big_id" "$running") == 409 ]] && jq -e '.code == "vm_running"' "$work/last" >/dev/null \
    || fail "a hardware change while it runs: $(cat "$work/last")"
grep -q "<vcpu placement='static'>2</vcpu>" <<<"$(vm "sudo virsh -c qemu:///system dumpxml --inactive $domain")" \
    || fail "the refused change reached libvirt"
renamed=$(jq '.name = "renamed vm"' <<<"$change")
[[ $(status PUT "/vms/$big_id" "$renamed") == 200 ]] || fail "rename while it runs: $(cat "$work/last")"
jq -e '.name == "renamed vm" and .state == "running"' "$work/last" >/dev/null || fail "rename: $(cat "$work/last")"
big="renamed vm"
change=$renamed

say "10. The ISO is ejected and inserted while it runs"
# What the running guest has in its drive sdi: the file, or "-".
in_drive() {
    vm "sudo virsh -c qemu:///system domblklist $domain --details" | awk '$3 == "sdi" { print $4 }'
}
[[ $(in_drive) == "$isos/$image.iso" ]] || fail "the drive holds $(in_drive)"
ejected=$(jq '.drives = [{slot: 0, image: null}]' <<<"$change")
[[ $(status PUT "/vms/$big_id" "$ejected") == 200 ]] || fail "eject: $(cat "$work/last")"
[[ $(in_drive) == "-" ]] || fail "still in the running guest's drive: $(in_drive)"
if vm "sudo virsh -c qemu:///system dumpxml --inactive $domain" | grep -q "$isos/$image.iso"; then
    fail "still in the definition"
fi
[[ $(status PUT "/vms/$big_id" "$change") == 200 ]] || fail "insert: $(cat "$work/last")"
[[ $(in_drive) == "$isos/$image.iso" ]] || fail "not in the running guest's drive: $(in_drive)"
vm "sudo virsh -c qemu:///system dumpxml --inactive $domain" | grep -q "<source file='$isos/$image.iso'/>" \
    || fail "not in the definition"

say "11. Its load comes on the live WebSocket"
sample=""
for attempt in 1 2; do
    if sample=$(NODE_NO_WARNINGS=1 NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-stats-probe.mjs" \
        "wss://${base#https://}/api/v1/ws" "$session" "$base" "$big_id" 2>&1); then
        break
    fi
    note "attempt $attempt: $sample"
done
load=$(jq -c --arg id "$big_id" '.vms[] | select(.id == $id)' <<<"$sample") || fail "no load: $sample"
jq -e '(.cpu_percent | type) == "number" and .memory_bytes == 536870912' <<<"$load" >/dev/null || fail "the load: $load"
jq -e '.running >= 1 and .total >= 1 and .host_cores >= 1 and .memory_assigned_bytes >= 536870912' <<<"$sample" >/dev/null \
    || fail "the overview: $sample"
note "CPU $(jq -r .cpu_percent <<<"$load") %, memory used $(jq -r '.memory_used_bytes // "unknown"' <<<"$load"), in $(jq -r '.received_bytes_per_second // "not counted"' <<<"$load") B/s"

say "12. The ISO stays while the VM has it; the VM goes, then the images"
[[ $(status DELETE "/vms/images/$image") == 409 ]] && jq -e --arg n "$big" '.code == "image_in_use" and (.detail | contains($n))' \
    "$work/last" >/dev/null || fail "deleted while in use: $(cat "$work/last")"
vm "sudo test -f $isos/$image.iso" || fail "the file went"
[[ $(status POST "/vms/$big_id/force-off") == 200 ]] || fail "force off: $(cat "$work/last")"
[[ $(status POST "/vms/$big_id/delete" "$(jq -n --arg n "$big" '{confirm: $n}')") == 204 ]] || fail "delete: $(cat "$work/last")"
for disk in 0 1; do
    if vm "sudo zfs list -H -o name $pool/zendiloos.vms/$big_id-disk$disk" >/dev/null 2>&1; then fail "disk $disk stayed"; fi
done
vm "sudo test -f $isos/$image.iso" || fail "the image went with the VM"
for gone in "$image" "$uploaded"; do
    [[ $(status DELETE "/vms/images/$gone") == 204 ]] || fail "delete image $gone: $(cat "$work/last")"
    vm "sudo test ! -e $isos/$gone.iso" || fail "the file of $gone stayed"
done
for action in vms.image_add vms.image_upload vms.change vms.image_delete; do
    call GET /logs/audit | jq -e --arg a "$action" '[.. | strings | select(. == $a)] | length > 0' >/dev/null \
        || fail "no audit entry $action"
done
delete_share

say "13. Nothing of libvirt's networking came, and zendiloOS's rules are as they were"
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

say "14. A second location of the VMs' storage goes on another pool"
plugin=$(call GET /vms/plugin)
first=$(jq -r '.storage.pool' <<<"$plugin")
second=${ZENDILOOS_VMS_POOL2:-$(jq -r '[.pools[].pool] - [.locations[].pool] | .[0] // empty' <<<"$plugin")}
if [[ -z $second ]]; then
    # One of the test's earlier runs may have left it, or someone added it.
    second=$(jq -r --arg f "$first" '[.locations[].pool | select(. != $f)][0] // empty' <<<"$plugin")
fi
[[ -n $second && $second != "$first" ]] || fail "no second pool for a location (the VM has $(jq -c '[.pools[].pool]' <<<"$plugin")): create one first"
if jq -e --arg p "$second" '.locations | any(.pool == $p)' <<<"$plugin" >/dev/null; then
    note "$second holds a location already; it stays after the test"
else
    [[ $(status POST /vms/locations "$(jq -n --arg p "$second" '{pool: $p}')") == 200 ]] \
        || fail "adding a location on $second: $(cat "$work/last")"
    location_added=$second
fi
plugin=$(call GET /vms/plugin)
jq -e --arg f "$first" --arg s "$second" '.storage.pool == $f and (.locations | map(.pool) | index($f) == 0 and index($s) != null)' \
    <<<"$plugin" >/dev/null || fail "the locations: $(jq -c '{storage, locations}' <<<"$plugin")"
volume2=$(jq -r --arg s "$second" '.locations[] | select(.pool == $s) | .volume' <<<"$plugin")
vm "findmnt -n -o SOURCE /volume$volume2/@vms" | grep -qx "$second/zendiloos.vms" || fail "/volume$volume2/@vms is not mounted"
vm "sudo zfs get -H -o value setuid,devices,exec $second/zendiloos.vms" | tr '\n' ' ' | grep -q '^off off off' \
    || fail "the second dataset allows setuid, devices or exec"
drop_in=$(vm 'sudo cat /etc/systemd/system/libvirtd.service.d/zendiloos.conf')
grep -q "RequiresMountsFor=/volume$volume/@vms" <<<"$drop_in" && grep -q "RequiresMountsFor=/volume$volume2/@vms" <<<"$drop_in" \
    || fail "libvirt does not wait for both locations: $drop_in"
for held in "$first" "$second"; do
    code=$(status POST "/storage/pools/$held/destroy" "$(jq -n --arg p "$held" '{confirm: $p}')")
    [[ $code == 409 ]] && jq -e '.code == "pool_in_use" and (.detail | contains("virtual machines"))' "$work/last" >/dev/null \
        || fail "deleting $held with a location on it was not refused: $code $(cat "$work/last")"
done
listed=$(call GET /vms/locations)
jq -e --arg f "$first" --arg s "$second" '(.[0].pool == $f and .[0].library) and any(.[]; .pool == $s and (.library | not) and .health != null)' \
    <<<"$listed" >/dev/null || fail "the locations listed: $listed"
note "$first (volume $volume, the library's) and $second (volume $volume2)"

say "15. A VM with a disk on each location, a card on an internal network and one directly on the NAS's card"
setup=$(call GET /network/setup)
jq -e '.managed' <<<"$setup" >/dev/null || fail "systemd-networkd does not manage the network here: run just vm-network-test first"
[[ $(jq -c .pending <<<"$setup") == null ]] || fail "a change of the network waits for confirmation: $(jq -c .pending <<<"$setup")"
if ! jq -e --arg n "$internal" '.devices | any(.name == $n)' <<<"$setup" >/dev/null; then
    code=$(network_change POST /network/devices "$(jq -n --arg n "$internal" '{kind: "bridge", name: $n, members: []}')")
    [[ $code == 202 ]] || fail "making the internal network: $code $(cat "$work/last")"
fi
internal_made=yes
wait_for "the internal network to be offered" \
    "call GET /vms/plugin | jq -e --arg n '$internal' '.internal_networks | index(\$n) != null'" 60
call GET /network/setup | jq -e --arg n "$internal" '.internal | index($n) != null' >/dev/null \
    || fail "the Network page does not name $internal an internal network"
wait_for "the NAS to keep itself off $internal" "vm 'sudo nft list table bridge zendiloos_internal' | grep -E 'ibrname .*$internal'" 60
vm 'systemctl is-active --quiet zendiloos-internal-networks.service' || fail "the internal networks' unit is not active"
rules=$(vm 'sudo nft list table bridge zendiloos_internal')
grep -qE "hook input .*" <<<"$rules" && grep -qE "obrname .*$internal.* drop" <<<"$rules" || fail "the bridge table: $rules"
sleep 3
[[ -z $(vm "ip -o addr show dev $internal") ]] || fail "the NAS has an address on $internal: $(vm "ip -o addr show dev $internal")"
plugin=$(call GET /vms/plugin)
route_card=$(vm "ip -o route show default" | awk '{ for (i = 1; i < NF; i++) if ($i == "dev") { print $(i + 1); exit } }')
card=$(jq -r --arg r "$route_card" '(.cards | if index($r) != null then $r else .[0] end) // empty' <<<"$plugin")
[[ -n $card ]] || fail "no wired card for a VM to go on directly: $(jq -c .cards <<<"$plugin")"
note "the internal network $internal, the card $card"
new=$(jq -n --arg n "$places" --arg s "$second" --arg i "$internal" --arg c "$card" '{name: $n, os: "linux", cpus: 1,
    memory_mib: 256, firmware: "bios",
    disks: [{slot: 0, size_gib: 1, bus: "virtio"}, {slot: 1, size_gib: 2, bus: "virtio", pool: $s}],
    nics: [{slot: 0, model: "virtio", network: {kind: "internal", network: $i}},
           {slot: 1, model: "virtio", network: {kind: "direct", card: $c}}],
    drives: [], boot: [{kind: "disk", slot: 0}], autostart: false}')
[[ $(status POST /vms "$(jq '.nics[1].network.card = "lo"' <<<"$new")") == 422 ]] || fail "a card directly on lo: $(cat "$work/last")"
[[ $(status POST /vms "$(jq '.disks[1].pool = "nowhere"' <<<"$new")") == 422 ]] || fail "a disk on no location: $(cat "$work/last")"
[[ $(status POST /vms "$new") == 202 ]] || fail "create: $(cat "$work/last")"
places_id=$(jq -er .vm.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 300
domain="zendiloos-$places_id"
vmjson=$(call GET "/vms/$places_id")
jq -e --arg f "$first" --arg s "$second" '.disks[0].pool == $f and .disks[1].pool == $s' <<<"$vmjson" >/dev/null \
    || fail "the disks' locations: $(jq -c .disks <<<"$vmjson")"
[[ $(vm "sudo zfs get -H -p -o value volsize,volmode $first/zendiloos.vms/$places_id-disk0" | tr '\n' ' ') == "$((1 << 30)) dev " ]] \
    || fail "disk 0 on $first"
[[ $(vm "sudo zfs get -H -p -o value volsize,volmode $second/zendiloos.vms/$places_id-disk1" | tr '\n' ' ') == "$((2 << 30)) dev " ]] \
    || fail "disk 1 on $second"
if vm "sudo zfs list -H -o name $first/zendiloos.vms/$places_id-disk1" >/dev/null 2>&1; then fail "disk 1 is on $first too"; fi
xml=$(vm "sudo virsh -c qemu:///system dumpxml --inactive $domain")
grep -q "<source dev='/dev/zvol/$second/zendiloos.vms/$places_id-disk1'/>" <<<"$xml" || fail "disk 1 is not on $second: $xml"
grep -A3 "<interface type='bridge'>" <<<"$xml" | grep -q "<source bridge='$internal'/>" || fail "no card on $internal: $xml"
grep -A3 "<interface type='direct'>" <<<"$xml" | grep -q "<source dev='$card' mode='bridge'/>" || fail "no card directly on $card: $xml"
for mac in $(jq -r '.nics[].mac' <<<"$vmjson"); do
    grep -q "<mac address='$mac'/>" <<<"$xml" || fail "no card with $mac: $xml"
done
if grep -q "type='network'" <<<"$xml"; then fail "a network of libvirt's: $xml"; fi
# Moved to the first location: refused, nothing changed.
moved=$(jq '{name, cpus, memory_mib, firmware, disks: (.disks | map(.pool = "'"$first"'")),
    nics: [.nics[] | {slot, model, network}], drives, boot, autostart}' <<<"$vmjson")
[[ $(status PUT "/vms/$places_id" "$moved") == 422 ]] || fail "a disk moved to another location: $(cat "$work/last")"
# Kept as they are, sent back as GET shows them: no change of the hardware, the addresses kept.
same=$(jq '{name, cpus, memory_mib, firmware, disks, nics: [.nics[] | {slot, model, network}], drives, boot, autostart}' <<<"$vmjson")
[[ $(status PUT "/vms/$places_id" "$same") == 200 ]] || fail "the same settings: $(cat "$work/last")"
same_cards "$places_id" "$vmjson"
# The location holds its disk: it stays.
code=$(status POST "/vms/locations/$second/delete" "$(jq -n --arg p "$second" '{confirm: $p}')")
[[ $code == 409 ]] && jq -e --arg n "$places" '.code == "location_in_use" and (.detail | contains($n))' "$work/last" >/dev/null \
    || fail "a location in use was removed: $code $(cat "$work/last")"
vm "sudo zfs list -H -o name $second/zendiloos.vms/$places_id-disk1" >/dev/null || fail "the disk on $second went"
# The card a VM is on directly is not taken into a bridge.
code=$(status POST /network/devices/plan "$(jq -n --arg c "$card" '{action: "make", device: {kind: "bridge", name: "ztestbr9", members: [$c]}}')")
[[ $code == 409 ]] && jq -e --arg n "$places" '.code == "device_in_use" and (.detail | contains($n))' "$work/last" >/dev/null \
    || fail "a card a VM is on directly would go into a bridge: $code $(cat "$work/last")"
listed=$(call GET /vms/locations)
jq -e --arg s "$second" --arg id "$places_id" '.[] | select(.pool == $s) | .vms | any(.id == $id and .disks == [1] and .size_bytes == 2147483648)' \
    <<<"$listed" >/dev/null || fail "the VM on $second is not listed: $listed"
# Started: its tap in the internal bridge, a macvtap on the card.
if [[ $(status POST "/vms/$places_id/start") != 200 ]]; then
    note "libvirt: $(vm "sudo tail -n 5 /var/log/libvirt/qemu/$domain.log" 2>&1)"
    fail "start: $(cat "$work/last")"
fi
wait_for "the VM's tap in $internal" "vm 'ip -o link show master $internal' | grep -q ." 30
vm "ip -o -d link show type macvtap" | grep -q "@$card" || fail "no macvtap on $card: $(vm 'ip -o -d link show type macvtap')"
[[ -z $(vm "ip -o addr show dev $internal") ]] || fail "the NAS got an address on $internal"
[[ $(status POST "/vms/$places_id/force-off") == 200 ]] || fail "force off: $(cat "$work/last")"
wait_for "the VM to be off" "[[ \$(vm_state $places_id) == shut_off ]]" 30
note "$domain: disks on $first and $second, cards on $internal and directly on $card"

say "16. The log shows the test's VM events"
log=$(call GET "/logs/audit?prefix=vms.&search=$(urlencode "$places")&limit=50")
for action in vms.create vms.change vms.start vms.force_off; do
    jq -e --arg a "$action" --arg n "$places" '.entries | any(.action == $a and .target == $n and .actor != null)' <<<"$log" >/dev/null \
        || fail "no $action of $places in the log: $log"
done
jq -e '.entries | all(.action | startswith("vms."))' <<<"$log" >/dev/null || fail "other entries in the VMs' log: $log"
if [[ -n $location_added ]]; then
    call GET "/logs/audit?prefix=vms.&search=$(urlencode "$second")" | jq -e '.entries | any(.action == "vms.location_add")' >/dev/null \
        || fail "no vms.location_add of $second in the log"
fi
[[ $(status GET "/logs/audit?search=$(printf 'x%.0s' $(seq 1 101))") == 422 ]] || fail "a search of 101 characters"

say "17. The VM goes, then the internal network and the location the test added"
[[ $(status POST "/vms/$places_id/delete" "$(jq -n --arg n "$places" '{confirm: $n}')") == 204 ]] || fail "delete: $(cat "$work/last")"
for zvol in "$first/zendiloos.vms/$places_id-disk0" "$second/zendiloos.vms/$places_id-disk1"; do
    if vm "sudo zfs list -H -o name $zvol" >/dev/null 2>&1; then fail "$zvol stayed"; fi
done
code=$(network_change DELETE "/network/devices/$internal")
[[ $code == 202 ]] || fail "removing the internal network: $code $(cat "$work/last")"
internal_made=""
wait_for "the bridge table to go" "! vm 'sudo nft list table bridge zendiloos_internal' 2>/dev/null | grep -q '$internal'" 60
if [[ -n $location_added ]]; then
    code=$(status POST "/vms/locations/$second/delete" '{"confirm":"wrong"}')
    [[ $code == 422 ]] || fail "removed without the pool's name: $code $(cat "$work/last")"
    [[ $(status POST "/vms/locations/$second/delete" "$(jq -n --arg p "$second" '{confirm: $p}')") == 204 ]] \
        || fail "removing the location: $(cat "$work/last")"
    location_added=""
    if vm "sudo zfs list -H -o name $second/zendiloos.vms" >/dev/null 2>&1; then fail "$second/zendiloos.vms stayed"; fi
    vm "sudo test ! -e '/etc/systemd/system/volume$volume2-\\x40vms.mount'" || fail "the location's mount unit stayed"
    if vm 'sudo cat /etc/systemd/system/libvirtd.service.d/zendiloos.conf' | grep -q "/volume$volume2/@vms"; then
        fail "libvirt still waits for the removed location"
    fi
    vm 'systemctl is-active --quiet libvirtd.service' || fail "libvirtd stopped with the location"
    call GET /vms/plugin | jq -e --arg s "$second" '.locations | all(.pool != $s)' >/dev/null || fail "the location is still listed"
fi

say "All virtual machine tests passed"
