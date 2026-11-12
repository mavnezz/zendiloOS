#!/usr/bin/env bash
# Snapshots, backups and restores of virtual machines, a TPM and Secure
# Boot, and the host's devices, end to end in the second test VM (nested
# KVM), through the HTTPS API, with virsh and zfs inside the VM to look:
#
#   1. the plugin is installed (again, if swtpm came only later) and its
#      storage set up; swtpm and OVMF with Microsoft's keys are there
#   2. the hardware check says whether the IOMMU is on and what the
#      kernel's command line says; /vms/devices lists the host's devices;
#      without an IOMMU a VM with a PCI device is refused (the test VM has
#      none, so PCI passthrough itself is skipped with a note)
#   3. a VM with a TPM and Secure Boot: its domain has the TPM (swtpm) and
#      Secure Boot with SMM; started, swtpm runs for it, libvirt took
#      OVMF_CODE_4M.ms.fd and OVMF_VARS_4M.ms.fd, and its UEFI variables
#      hold Microsoft's keys (virt-fw-vars says Secure Boot is on, where it
#      is installed); Secure Boot switched off, its variables start afresh
#      without them
#   4. a VM with two disks, data written into its zvols from the host (no
#      guest needed): a snapshot of both at once with a name, more data, a
#      second snapshot; a rollback to the first is refused while newer ones
#      would go, and done once that is agreed: the disks hold what they
#      held, by checksum; a snapshot of the running VM is crash-consistent
#      without a guest agent; schedules are kept
#   5. a backup into a share of the test's: disk images compressed with
#      zstd, the UEFI variables and a manifest beside them, no snapshot or
#      unit left; restored as a new VM, its disks hold the same, by
#      checksum; changed and restored over the old VM (off), its disks are
#      back; a backup canceled leaves no manifest and no unit; the backup
#      is deleted with its files
#   6. USB: if the test VM has USB devices (scripts/vm.sh start with
#      ZENDILOOS_VM_USB=1 and a kernel with USB; the cloud kernel has none),
#      one is given to a VM by vendor and product and pulled out while it
#      runs; otherwise skipped with a note
#
# Needs the VM running with the package installed and set up, a pool
# zendiloOS made (ZENDILOOS_VMS_POOL picks it for the storage if it is not
# set up yet), the network for apt, jq and sha256sum on this machine;
# account and password as for vm-storage-test.sh. Runs in the second VM by
# default. The test's VMs, backups and share go, also when it fails.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
secure="ztest secure"
disks="ztest disks"
copy="ztest copy"
usb_name="ztest usb"
share="ztestvmbak"
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

job_end() {
    local id=$1 seconds=${2:-900}
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")'
}

job_succeeded() {
    local state
    state=$(job_end "$@")
    [[ $state == succeeded* ]] || fail "job $1: $state"
}

vm_id() { call GET /vms | jq -r --arg n "$1" '.[] | select(.name == $n) | .id'; }
vm_state() { call GET "/vms/$1" | jq -r .state; }

force_off() {
    [[ $(vm_state "$1") == shut_off ]] || call POST "/vms/$1/force-off" >/dev/null
    wait_for "the VM to be off" "[[ \$(vm_state $1) == shut_off ]]" 30
}

delete_vm() {
    local id
    id=$(vm_id "$1")
    [[ -n $id ]] || return 0
    call POST "/vms/$id/backups/cancel" >/dev/null 2>&1 || true
    [[ $(vm_state "$id") == shut_off ]] || call POST "/vms/$id/force-off" >/dev/null
    call POST "/vms/$id/delete" "$(jq -n --arg n "$1" '{confirm: $n}')" >/dev/null
}

delete_share() {
    local id
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
    done
}

# The test's backups: those of its VMs, by their names.
delete_backups() {
    local id
    for id in $(call GET /vms/backups | jq -r '.[] | select(.vm_name | startswith("ztest")) | .id'); do
        call POST "/vms/backups/$id/delete" >/dev/null || true
    done
}

clean() {
    delete_vm "$secure"
    delete_vm "$disks"
    delete_vm "$copy"
    delete_vm "$usb_name"
    delete_backups
    delete_share
    "$root/scripts/vm.sh" usb-detach ztestusb >/dev/null 2>&1 || true
    return 0
}

cleanup() {
    set +e
    [[ -s $work/admin.csrf ]] && clean
    rm -rf "$work"
}
trap cleanup EXIT

# The first MiBs of a zvol, as a checksum: what the test wrote is there.
zvol_sum() { vm "sudo dd if=/dev/zvol/$1 bs=1M count=16 status=none | sha256sum | cut -d' ' -f1"; }

# Writes 8 MiB of random data at the start of a zvol, as a guest would.
scribble() { vm "sudo dd if=/dev/urandom of=/dev/zvol/$1 bs=1M count=8 conv=fsync status=none"; }

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
command -v sha256sum >/dev/null || fail "sha256sum is needed on this machine"
me=$(call GET /auth/session | jq -er .account.id) || fail "no account"

say "Cleaning up an earlier run"
clean

say "1. The plugin with swtpm, its storage set up"
plugin=$(call GET /vms/plugin)
jq -e '.hardware.kvm == true' <<<"$plugin" >/dev/null || fail "no KVM in the VM (nested KVM off?): $plugin"
if jq -e '.installed == false or .hardware.swtpm == false' <<<"$plugin" >/dev/null; then
    [[ $(status POST /vms/plugin/install) == 202 ]] || fail "install: $(cat "$work/last")"
    job_succeeded "$(jq -r .id "$work/last")" 1800
fi
for package in swtpm swtpm-tools ovmf; do
    vm "dpkg-query -W -f='\${Status}' $package 2>/dev/null | grep -q 'install ok installed'" || fail "$package is not installed"
done
vm 'test -f /usr/share/OVMF/OVMF_CODE_4M.ms.fd && test -f /usr/share/OVMF/OVMF_VARS_4M.ms.fd' \
    || fail "Debian's OVMF with Microsoft's keys is not there"
plugin=$(call GET /vms/plugin)
jq -e '.hardware.swtpm and .hardware.secure_boot' <<<"$plugin" >/dev/null || fail "swtpm or Secure Boot not seen: $plugin"
pool=$(jq -r '.storage.pool // empty' <<<"$plugin")
if [[ -z $pool ]]; then
    pool=${ZENDILOOS_VMS_POOL:-$(jq -r '.pools[0].pool // empty' <<<"$plugin")}
    [[ -n $pool ]] || fail "no pool for the VMs' storage: create one first"
    [[ $(status POST /vms/plugin/storage "{\"pool\":\"$pool\"}") == 200 ]] || fail "storage: $(cat "$work/last")"
    plugin=$(call GET /vms/plugin)
fi
note "storage on $pool"

say "2. The IOMMU and the host's devices"
devices=$(call GET /vms/devices)
jq -e '.iommu | has("on") and has("cmdline")' <<<"$devices" >/dev/null || fail "no IOMMU in the answer: $devices"
note "IOMMU on: $(jq -r .iommu.on <<<"$devices"), kernel command line: $(jq -r '.iommu.cmdline | join(" ") | if . == "" then "nothing of it" else . end' <<<"$devices"), CPU: $(jq -r '.iommu.cpu_vendor // "?"' <<<"$devices")"
[[ $(jq -r .iommu.on <<<"$devices") == $(jq -r .hardware.iommu <<<"$plugin") ]] || fail "the hardware check and the devices disagree"
note "$(jq '.pci | length' <<<"$devices") PCI devices, $(jq '[.pci[] | select(.needed != null)] | length' <<<"$devices") of them the host's, $(jq '.usb | length' <<<"$devices") USB devices"
jq -e '[.pci[] | select(.needed.kind == "disk")] | length > 0' <<<"$devices" >/dev/null \
    || fail "no disk controller is the host's: $(jq -c '.pci' <<<"$devices")"
if jq -e '.iommu.on == false' <<<"$devices" >/dev/null; then
    address=$(jq -r '[.pci[] | select(.needed == null)][0].address // "0000:00:01.0"' <<<"$devices")
    body=$(jq -n --arg a "$address" '{name: "ztest pci", os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi",
        disks: [{slot: 0, size_gib: 1, bus: "virtio"}], nics: [], drives: [], boot: [{kind: "disk", slot: 0}], pci: [$a]}')
    [[ $(status POST /vms "$body") == 422 ]] && grep -q 'IOMMU is off' "$work/last" \
        || fail "a PCI device without an IOMMU: $(cat "$work/last")"
    note "skipped: handing a PCI device over needs an IOMMU, which the test VM has not"
else
    note "skipped: an IOMMU is on, but handing a device of the test VM over is left to a machine one may lose"
fi

say "3. A VM with a TPM and Secure Boot"
body=$(jq -n --arg n "$secure" '{name: $n, os: "windows", cpus: 1, memory_mib: 512, firmware: "uefi",
    disks: [{slot: 0, size_gib: 1, bus: "sata"}], nics: [], drives: [], boot: [{kind: "disk", slot: 0}],
    tpm: true, secure_boot: true}')
[[ $(status POST /vms "$(jq '.firmware = "bios"' <<<"$body")") == 422 ]] || fail "Secure Boot with BIOS: $(cat "$work/last")"
[[ $(status POST /vms "$body") == 202 ]] || fail "create: $(cat "$work/last")"
sid=$(jq -er .vm.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 300
domain="zendiloos-$sid"
xml=$(vm "sudo virsh -c qemu:///system dumpxml --inactive $domain")
grep -q "<tpm model='tpm-crb'>" <<<"$xml" && grep -q "<backend type='emulator' version='2.0'" <<<"$xml" \
    || fail "no TPM: $xml"
grep -q "name='secure-boot'" <<<"$xml" && grep -q "<smm state='on'/>" <<<"$xml" || fail "no Secure Boot with SMM: $xml"
grep -q "name='org.qemu.guest_agent.0'" <<<"$xml" || fail "no guest agent's channel: $xml"
[[ $(status POST "/vms/$sid/start") == 200 ]] || fail "start: $(cat "$work/last") $(vm "sudo tail -n 5 /var/log/libvirt/qemu/$domain.log" 2>&1)"
xml=$(vm "sudo virsh -c qemu:///system dumpxml $domain")
grep -q 'OVMF_CODE_4M.ms.fd' <<<"$xml" && grep -q "template='/usr/share/OVMF/OVMF_VARS_4M.ms.fd'" <<<"$xml" \
    || fail "not OVMF with Microsoft's keys: $(grep -E 'loader|nvram' <<<"$xml")"
uuid=$(vm "sudo virsh -c qemu:///system domuuid $domain" | tr -d '[:space:]')
vm "pgrep -f 'swtpm socket.*$uuid' >/dev/null" || fail "swtpm does not run for the VM"
vm "sudo test -d /var/lib/libvirt/swtpm/$uuid/tpm2" || fail "no TPM state in /var/lib/libvirt/swtpm/$uuid"
nvram="/var/lib/libvirt/qemu/nvram/${domain}_VARS.fd"
vm "sudo grep -a -q 'Microsoft Corporation KEK CA' $nvram" || fail "Microsoft's keys are not in the UEFI variables"
if vm 'command -v virt-fw-vars >/dev/null'; then
    vm "sudo virt-fw-vars --input $nvram --print 2>/dev/null | grep -i -q 'SecureBootEnable.*\\(on\\|01\\)'" \
        || note "virt-fw-vars does not say Secure Boot is on: $(vm "sudo virt-fw-vars --input $nvram --print 2>/dev/null | grep -i secureboot" || true)"
else
    note "virt-fw-vars (python3-virt-firmware) is not installed: the variables were checked for Microsoft's keys only"
fi
force_off "$sid"
vm_json=$(call GET "/vms/$sid")
change=$(jq '{name, cpus, memory_mib, firmware, disks, nics: [.nics[] | {slot, model, network}], drives, boot,
    autostart, usb, pci, tpm, secure_boot: false}' <<<"$vm_json")
[[ $(status PUT "/vms/$sid" "$change") == 200 ]] || fail "Secure Boot off: $(cat "$work/last")"
vm "sudo test -f /etc/zendiloos/vms/$domain.reset-nvram" || fail "the variables are not to start afresh"
[[ $(status POST "/vms/$sid/start") == 200 ]] || fail "start without Secure Boot: $(cat "$work/last")"
if vm "sudo grep -a -q 'Microsoft Corporation KEK CA' $nvram"; then fail "the old variables stayed"; fi
force_off "$sid"
note "TPM state in /var/lib/libvirt/swtpm/$uuid, variables from the Microsoft template, then afresh"

say "4. Snapshots of two disks at once, rolled back, compared"
body=$(jq -n --arg n "$disks" '{name: $n, os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi",
    disks: [{slot: 0, size_gib: 1, bus: "virtio"}, {slot: 1, size_gib: 1, bus: "virtio"}], nics: [], drives: [],
    boot: [{kind: "disk", slot: 0}]}')
[[ $(status POST /vms "$body") == 202 ]] || fail "create: $(cat "$work/last")"
did=$(jq -er .vm.id "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 300
d0="$pool/zendiloos.vms/$did-disk0"
d1="$pool/zendiloos.vms/$did-disk1"
scribble "$d0"
scribble "$d1"
first0=$(zvol_sum "$d0")
first1=$(zvol_sum "$d1")
[[ $(status POST "/vms/$did/snapshots" '{"name":"before the change"}') == 201 ]] || fail "snapshot: $(cat "$work/last")"
first=$(jq -er .label "$work/last")
jq -e '.consistency == "off" and .disks == [0, 1]' "$work/last" >/dev/null || fail "the snapshot: $(cat "$work/last")"
for zvol in "$d0" "$d1"; do
    props=$(vm "sudo zfs get -H -o value zendiloos:note,zendiloos:consistency $zvol@$first" | tr '\n' ' ')
    [[ $props == "before the change off " ]] || fail "$zvol@$first: $props"
done
scribble "$d0"
sleep 1
[[ $(status POST "/vms/$did/snapshots" '{}') == 201 ]] || fail "second snapshot: $(cat "$work/last")"
second=$(jq -er .label "$work/last")
scribble "$d1"
[[ $(zvol_sum "$d0") != "$first0" ]] || fail "the zvol did not change"
[[ $(status POST "/vms/$did/snapshots/$first/rollback" '{}') == 409 ]] && jq -e '.code == "newer_snapshots"' "$work/last" >/dev/null \
    || fail "a rollback past a newer snapshot: $(cat "$work/last")"
jq -e --arg s "$second" '.detail == $s' "$work/last" >/dev/null || fail "the newer ones are not named: $(cat "$work/last")"
[[ $(status POST "/vms/$did/snapshots/$first/rollback" '{"newer":true}') == 200 ]] || fail "rollback: $(cat "$work/last")"
[[ $(zvol_sum "$d0") == "$first0" && $(zvol_sum "$d1") == "$first1" ]] || fail "the disks do not hold what they held"
if vm "sudo zfs list -H -t snapshot $d0@$second" >/dev/null 2>&1; then fail "the newer snapshot stayed"; fi
note "both disks back to $first, the newer snapshot gone"
[[ $(status POST "/vms/$did/start") == 200 ]] || fail "start: $(cat "$work/last")"
[[ $(status POST "/vms/$did/snapshots/$first/rollback" '{}') == 409 ]] || fail "a rollback while it runs: $(cat "$work/last")"
[[ $(status POST "/vms/$did/snapshots" '{"name":"running"}') == 201 ]] || fail "snapshot while it runs: $(cat "$work/last")"
running=$(jq -er .label "$work/last")
jq -e '.consistency == "crash" or .consistency == "frozen"' "$work/last" >/dev/null || fail "while it runs: $(cat "$work/last")"
note "while it runs: $(jq -r .consistency "$work/last")"
[[ $(status POST "/vms/$did/snapshots/$running/delete") == 204 ]] || fail "delete: $(cat "$work/last")"
force_off "$did"
[[ $(status PUT "/vms/$did/snapshots/schedules" '{"schedules":[{"frequency":"daily","keep":7}]}') == 200 ]] \
    || fail "schedules: $(cat "$work/last")"
call GET "/vms/$did/snapshots" | jq -e '.schedules == [{"frequency":"daily","keep":7}]' >/dev/null || fail "the schedules"
[[ $(status PUT "/vms/$did/snapshots/schedules" '{"schedules":[]}') == 200 ]] || fail "no schedules"

say "5. A backup into a share, restored as a new VM and over the old one"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name: $n, pool: $pool, access: [{kind: "account", id: $me, access: "read_write"}]}')" | jq -er .id) \
    || fail "the test's share"
volume=$(call GET /shares | jq -er --arg s "$share" '.[] | select(.name == $s) | .volume')
target=$(jq -n --argjson s "$share_id" '{target: {kind: "share", share_id: $s}, folder: "vm-backups"}')
# The VM's snapshots before: the one from step 4 stays, the backup's own goes.
snapshots_before=$(vm "sudo zfs list -H -t snapshot -o name -r $pool/zendiloos.vms" | grep "$did" || true)
[[ $(status POST "/vms/$did/backups" "$target") == 202 ]] || fail "backup: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")" 900
backup=$(call GET /vms/backups | jq -c --arg id "$did" '[.[] | select(.vm == $id)][0]')
backup_id=$(jq -er .id <<<"$backup")
folder="/volume$volume/$share/$(jq -r .folder <<<"$backup")"
note "into $folder: $(jq -r '[.files[].part.kind] | join(", ")' <<<"$backup"), $(jq -r .bytes <<<"$backup") bytes"
for file in disk0.raw.zst disk1.raw.zst nvram.fd manifest.json; do
    vm "sudo test -f '$folder/$file'" || fail "no $file in $folder"
done
mode=$(vm "sudo stat -c %a '$folder/disk0.raw.zst'")
(( (8#$mode & 8#022) == 0 )) || fail "the image may be written by others: $mode"
vm "sudo cat '$folder/manifest.json'" | jq -e --arg n "$disks" '.settings.name == $n and (.files | length) == 3' >/dev/null \
    || fail "the manifest: $(vm "sudo cat '$folder/manifest.json'")"
if vm 'command -v zstd >/dev/null'; then
    [[ $(vm "sudo zstd -dc '$folder/disk0.raw.zst' | head -c $((16 << 20)) | sha256sum | cut -d' ' -f1") == "$first0" ]] \
        || fail "the image does not hold the disk"
    note "Debian's zstd reads the image back"
fi
snapshots_after=$(vm "sudo zfs list -H -t snapshot -o name -r $pool/zendiloos.vms" | grep "$did" || true)
if [[ $snapshots_after != "$snapshots_before" ]]; then
    fail "the backup's snapshot stayed: before: $snapshots_before; after: $snapshots_after"
fi
[[ $(vm "sudo zfs get -H -o value snapdev $d0") == hidden ]] || fail "the snapshots' devices stayed"
[[ -z $(vm "systemctl list-units --all --plain --no-legend 'zendiloos-vm-*'") ]] || fail "a unit stayed"

[[ $(status POST "/vms/backups/$backup_id/restore" "$(jq -n --arg n "$copy" '{as: {kind: "new", name: $n}}')") == 202 ]] \
    || fail "restore as new: $(cat "$work/last")"
cid=$(jq -er .vm "$work/last")
job_succeeded "$(jq -r .job.id "$work/last")" 900
[[ $(zvol_sum "$pool/zendiloos.vms/$cid-disk0") == "$first0" && $(zvol_sum "$pool/zendiloos.vms/$cid-disk1") == "$first1" ]] \
    || fail "the new VM's disks do not hold the backup's"
vm "sudo test -f /var/lib/libvirt/qemu/nvram/zendiloos-${cid}_VARS.fd" || fail "the new VM has no UEFI variables"
[[ $(call GET "/vms/$cid" | jq -r '.nics | length') == 0 ]] || fail "the copy's cards"
note "the new VM $cid holds the same"

scribble "$d0"
[[ $(status POST "/vms/backups/$backup_id/restore" '{"as":{"kind":"over","confirm":"ztest"}}') == 422 ]] \
    || fail "over with the wrong name: $(cat "$work/last")"
[[ $(status POST "/vms/backups/$backup_id/restore" "$(jq -n --arg n "$disks" '{as: {kind: "over", confirm: $n}}')") == 202 ]] \
    || fail "restore over: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")" 900
[[ $(zvol_sum "$d0") == "$first0" ]] || fail "the old VM's disk is not back"
note "over the old VM: its disks are back"

[[ $(status POST "/vms/$sid/backups" "$target") == 202 ]] || fail "a second backup: $(cat "$work/last")"
cancel_job=$(jq -r .id "$work/last")
[[ $(status POST "/vms/$sid/backups/cancel") == 204 ]] || fail "cancel: $(cat "$work/last")"
state=$(job_end "$cancel_job" 300)
[[ $state == "failed canceled"* || $state == succeeded* ]] || fail "the canceled job: $state"
if [[ $state == "failed canceled"* ]]; then
    if [[ -n $(vm "sudo find /volume$volume/$share/vm-backups/$sid -name manifest.json") ]]; then
        fail "a canceled backup left a manifest"
    fi
    note "canceled; no manifest"
else
    note "the backup was done before the cancel came"
fi
wait_for "no unit of a VM" "[[ -z \$(vm \"systemctl list-units --all --plain --no-legend 'zendiloos-vm-*'\") ]]" 60

[[ $(status POST "/vms/backups/$backup_id/delete") == 204 ]] || fail "delete the backup: $(cat "$work/last")"
vm "sudo test ! -e '$folder'" || fail "the backup's files stayed"
for action in vms.snapshot vms.rollback vms.backup vms.restore vms.backup_delete; do
    call GET /logs/audit | jq -e --arg a "$action" '[.. | strings | select(. == $a)] | length > 0' >/dev/null \
        || fail "no audit entry $action"
done

say "6. USB passthrough"
"$root/scripts/vm.sh" usb-attach ztestusb >/dev/null 2>&1 || true
sleep 2
devices=$(call GET /vms/devices)
usb=$(jq -r '.usb[0].id // empty' <<<"$devices")
if [[ -z $usb ]]; then
    note "skipped: the test VM sees no USB device (its cloud kernel has no USB; start it with ZENDILOOS_VM_USB=1 and a kernel that has)"
else
    body=$(jq -n --arg n "$usb_name" --arg u "$usb" '{name: $n, os: "linux", cpus: 1, memory_mib: 256, firmware: "uefi",
        disks: [{slot: 0, size_gib: 1, bus: "virtio"}], nics: [], drives: [], boot: [{kind: "disk", slot: 0}], usb: [$u]}')
    [[ $(status POST /vms "$body") == 202 ]] || fail "a VM with $usb: $(cat "$work/last")"
    uid=$(jq -er .vm.id "$work/last")
    job_succeeded "$(jq -r .job.id "$work/last")" 300
    vendor=${usb%%:*}
    # Read whole first: grep -q stopping early breaks the pipe, which
    # pipefail counts.
    xml=$(vm "sudo virsh -c qemu:///system dumpxml zendiloos-$uid")
    grep -q "<vendor id='0x$vendor'/>" <<<"$xml" || fail "no hostdev of $usb"
    [[ $(status POST "/vms/$uid/start") == 200 ]] || fail "start with $usb: $(cat "$work/last")"
    change=$(call GET "/vms/$uid" | jq '{name, cpus, memory_mib, firmware, disks, nics: [], drives, boot, autostart,
        usb: [], pci, tpm, secure_boot}')
    [[ $(status PUT "/vms/$uid" "$change") == 200 ]] || fail "pulled out while it runs: $(cat "$work/last")"
    xml=$(vm "sudo virsh -c qemu:///system dumpxml zendiloos-$uid")
    if grep -q "<vendor id='0x$vendor'/>" <<<"$xml"; then
        fail "$usb is still in the running VM"
    fi
    note "$usb given by vendor and product, pulled out while it ran"
fi

say "Done: all checks passed"
