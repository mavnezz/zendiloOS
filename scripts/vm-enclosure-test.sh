#!/usr/bin/env bash
# Tests where each disk sits end to end in a test VM, through the HTTPS API,
# the real agent, udev and the VM's disks (4 SATA on q35's AHCI, 1 NVMe):
#
#   1. the machine (DMI "QEMU") is no model of the catalog, and the catalog
#      lists the UGREEN DXP4800 Plus
#   2. every disk says where it is plugged in (udev's ID_PATH)
#   3. a layout of one's own, a 1×4 SATA group bound by port (in the other
#      spelling, -ata-N for -ata-N.0) and an M.2 group with the NVMe bound
#      by serial number: each disk in its slot, an unbound slot empty, a
#      serial number not present missing
#   4. a disk blinks: the agent reads from it (its read count rises), never
#      writes, and stops when asked
#   5. the inventory keeps what was noted about a disk, and refuses to
#      forget one present
#   6. two disks reporting one serial number, as a USB dock's bridge does,
#      are two records of the inventory by serial number and port, and stay
#      exactly those two when one is pulled out and zendiloosd restarts
#
# Needs the VM started by a vm.sh with the monitor (for step 6), the package
# installed and set up; account and password as for vm-storage-test.sh. Runs
# in the second VM by default (the first holds a LAN test). Puts back the
# enclosure and the noted details, and forgets the dock's disks again (that
# their serial number is shared stays known).
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/jar" -X "$method" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf")")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

# The status code; the body lands in $work/last.
status() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -o "$work/last" -w '%{http_code}' -b "$work/jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/csrf")")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 check=$2 seconds=$3
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The inventory's records of the dock's disks (step 6): serial number and
# the disk present, if one is.
records() {
    call GET /storage/inventory \
        | jq -c --arg d "$dock" '[.[] | select(.serial | startswith($d)) | [.serial, .disk]] | sort'
}

# The inventory's path of the record $1.
record_path() { echo "/storage/inventory/$(jq -rn --arg s "$1" '$s | @uri')"; }

# Reads and writes completed on the disk, from the kernel's counters.
reads() { vm "cat /sys/block/$1/stat" | awk '{ print $1 }'; }
writes() { vm "cat /sys/block/$1/stat" | awk '{ print $5 }'; }

# What GET /storage/enclosure says, as the PUT that would choose it again.
as_choice() {
    jq -c 'if .source == "model" then
             { source: "model", model: .model.id,
               overrides: [.groups[].slots[] | select(.overridden) | { slot: .id, binding }] }
           elif .source == "custom" then
             { source: "custom",
               groups: [.groups[] | { id, name, rows, columns, orientation, form, numbering,
                                      slots: [.slots[] | { id, label, binding }] }] }
           else null end'
}

serial=ZDL-SATA-1
dock=ZDL-DOCK-1
docked=()
cleanup() {
    set +e
    for id in "${docked[@]}"; do "$root/scripts/vm.sh" removable-detach "$id" 2>/dev/null; done
    if (( ${#docked[@]} )); then
        sleep 3
        for record in $(records | jq -r '.[][0]'); do call DELETE "$(record_path "$record")" >/dev/null; done
    fi
    if [[ -s $work/enclosure ]]; then
        choice=$(as_choice <"$work/enclosure")
        if [[ $choice == null ]]; then
            call DELETE /storage/enclosure >/dev/null
        else
            call PUT /storage/enclosure "$choice" >/dev/null
        fi
    fi
    if [[ -s $work/noted ]]; then
        call PUT "/storage/inventory/$serial" \
            "$(jq -c '{ purchased, warranty_until, vendor, note }' "$work/noted")" >/dev/null
    fi
    for disk in "${disks[@]:-}"; do [[ -n $disk ]] && call DELETE "/storage/disks/$disk/locate" >/dev/null; done
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in"
call GET /storage/enclosure >"$work/enclosure"
jq -e '.machine' "$work/enclosure" >/dev/null || fail "no enclosure: $(cat "$work/enclosure")"

say "1. No model for QEMU; the catalog lists the DXP4800 Plus"
jq -e '.machine.vendor == "QEMU" and .detected == null' "$work/enclosure" >/dev/null \
    || fail "detected: $(jq -c '{ machine, detected: .detected.id }' "$work/enclosure")"
call GET /storage/enclosure/models >"$work/models"
jq -e '.[] | select(.id == "ugreen-dxp4800-plus" and .dmi_vendor == "UGREEN" and .dmi_product == "DXP4800 Plus")' \
    "$work/models" >/dev/null || fail "catalog: $(jq -c 'map(.id)' "$work/models")"
[[ $(jq '[.[] | select(.id == "ugreen-dxp4800-plus") | .groups[].slots[]] | length' "$work/models") == 7 ]] \
    || fail "the DXP4800 Plus has 7 slots"

say "2. Every disk says where it is plugged in"
call GET /storage/disks >"$work/disks"
disks=()
ports=()
for n in 1 2 3 4; do
    name=$(jq -er --arg s "ZDL-SATA-$n" '.[] | select(.serial == $s) | .name' "$work/disks") \
        || fail "no disk ZDL-SATA-$n"
    port=$(jq -er --arg s "ZDL-SATA-$n" '.[] | select(.serial == $s) | .id_path' "$work/disks") \
        || fail "$name has no port"
    [[ $port =~ ^pci-[0-9a-f:.]+-ata-$n(\.0)?$ ]] || fail "$name at $port"
    vm "test -e /dev/disk/by-path/$port" || fail "$port is no by-path link"
    disks+=("$name")
    ports+=("$port")
done
nvme=$(jq -er '.[] | select(.serial == "ZDL-NVME-1") | .name' "$work/disks") || fail "no NVMe disk"
jq -e --arg n "$nvme" '.[] | select(.name == $n) | .id_path | test("-nvme-1$")' "$work/disks" >/dev/null \
    || fail "$nvme has no port"

say "3. A layout of one's own: SATA by port, the NVMe by serial number"
slots=$(for n in 1 2 3 4; do
    # The other spelling: -ata-1 for -ata-1.0, and the other way round.
    port=${ports[n - 1]}
    if [[ $port == *.0 ]]; then port=${port%.0}; else port=$port.0; fi
    jq -n --arg id "sata$n" --arg label "SATA $n" --arg port "$port" \
        '{ id: $id, label: $label, binding: { kind: "port", value: $port } }'
done | jq -sc .)
layout=$(jq -nc --argjson sata "$slots" '{ source: "custom", groups: [
    { id: "sata", name: "SATA", rows: 1, columns: 4, orientation: "vertical", form: "inch_3_5",
      numbering: "left_to_right", slots: $sata },
    { id: "m2", name: "M.2", rows: 1, columns: 3, orientation: "horizontal", form: "m2",
      numbering: "left_to_right", slots: [
        { id: "m2-1", label: "M.2 1", binding: { kind: "serial", value: "ZDL-NVME-1" } },
        { id: "m2-2", label: "M.2 2", binding: null },
        { id: "m2-3", label: "M.2 3", binding: { kind: "serial", value: "ZDL-NOT-HERE" } } ] } ] }')
[[ $(status PUT /storage/enclosure "$layout") == 200 ]] || fail "layout: $(cat "$work/last")"
placed=$(call GET /storage/enclosure)
for n in 1 2 3 4; do
    jq -e --arg id "sata$n" --arg disk "${disks[n - 1]}" \
        '.groups[0].slots[] | select(.id == $id) | .state == "occupied" and .disk == $disk' <<<"$placed" >/dev/null \
        || fail "sata$n: $(jq -c --arg id "sata$n" '.groups[0].slots[] | select(.id == $id)' <<<"$placed")"
done
jq -e --arg disk "$nvme" '.groups[1].slots[0] | .state == "occupied" and .disk == $disk' <<<"$placed" >/dev/null \
    || fail "the NVMe by serial number: $(jq -c '.groups[1].slots[0]' <<<"$placed")"
jq -e '.groups[1].slots[1].state == "empty" and .groups[1].slots[2].state == "missing"' <<<"$placed" >/dev/null \
    || fail "empty and missing: $(jq -c '.groups[1].slots' <<<"$placed")"
jq -e --arg disk "${disks[0]}" '.unplaced | index($disk) == null' <<<"$placed" >/dev/null || fail "a placed disk unplaced"
jq -e '.source == "custom" and .chassis.style == "plain"' <<<"$placed" >/dev/null || fail "not the own layout"
bad=$(jq -c '.groups[0].slots[0].binding.value += "-part1"' <<<"$layout")
[[ $(status PUT /storage/enclosure "$bad") == 422 ]] || fail "a partition's path was taken"

say "4. A disk blinks: read from, never written, until stopped"
# One no pool uses, so nothing else reads or writes it meanwhile.
disk=$(jq -r '[.[] | select((.serial // "" | startswith("ZDL-SATA")) and .usage == "unused") | .name][0] // empty' \
    "$work/disks")
disk=${disk:-${disks[2]}}
before_reads=$(reads "$disk")
before_writes=$(writes "$disk")
[[ $(status POST "/storage/disks/$disk/locate" '{"seconds": 30}') == 200 ]] || fail "locate: $(cat "$work/last")"
now=$(date +%s)
# The VM's clock against the host's, a few seconds either way.
call GET /storage/locate | jq -e --arg d "$disk" --argjson now "$now" \
    '.[] | select(.disk == $d and .until > $now + 20 and .until < $now + 40)' >/dev/null || fail "not blinking"
sleep 4
during=$(reads "$disk")
(( during > before_reads + 3 )) || fail "$disk was hardly read: $before_reads, then $during"
[[ $(writes "$disk") == "$before_writes" ]] || fail "$disk was written to"
vm "sudo journalctl -u zendiloos-agent --since -1min --no-pager" | grep -q read_burst \
    || fail "no burst in the agent's log"
[[ $(status DELETE "/storage/disks/$disk/locate") == 204 ]] || fail "stop: $(cat "$work/last")"
call GET /storage/locate | jq -e --arg d "$disk" 'map(select(.disk == $d)) | length == 0' >/dev/null \
    || fail "still blinking"
sleep 2
stopped=$(reads "$disk")
sleep 3
(( $(reads "$disk") <= stopped + 2 )) || fail "$disk is still read after the stop"
[[ $(status POST "/storage/disks/$disk/locate" '{"seconds": 301}') == 422 ]] || fail "301 seconds taken"
[[ $(status POST /storage/disks/sdz/locate '{}') == 404 ]] || fail "a disk not there blinks"

say "5. The inventory keeps what was noted"
call GET /storage/inventory >"$work/inventory"
jq -e --arg s "$serial" --arg d "${disks[0]}" '.[] | select(.serial == $s and .disk == $d)' "$work/inventory" \
    >"$work/noted" || fail "$serial not in the inventory"
note='{"purchased": "2024-03-01", "warranty_until": "2027-02-28", "vendor": "Test shop", "note": "vm-enclosure-test"}'
[[ $(status PUT "/storage/inventory/$serial" "$note") == 200 ]] || fail "noting: $(cat "$work/last")"
call GET /storage/inventory | jq -e --arg s "$serial" \
    '.[] | select(.serial == $s) | .purchased == "2024-03-01" and .warranty_until == "2027-02-28"
       and .vendor == "Test shop" and .note == "vm-enclosure-test" and .first_seen <= .last_seen' >/dev/null \
    || fail "not kept: $(call GET /storage/inventory | jq -c --arg s "$serial" '.[] | select(.serial == $s)')"
[[ $(status PUT "/storage/inventory/$serial" '{"purchased": "2024-03-01", "warranty_until": "2024-01-01"}') == 422 ]] \
    || fail "a warranty ending before the purchase was taken"
[[ $(status DELETE "/storage/inventory/$serial") == 409 ]] || fail "a disk present was forgotten"

say "6. Two disks of one serial number, as in a USB dock: two records, whichever is there"
# Empty, so nothing mounts them; big enough to be disks.
for n in 0 1; do
    truncate -s 80M "$work/dock$n.img"
    "$root/scripts/vm.sh" removable-attach "$work/dock$n.img" "dock$n" rw "$dock"
    docked+=("dock$n")
done
wait_for "both disks" "[[ \$(records | jq 'map(select(.[1] != null)) | length') == 2 ]]" 30
both=$(records)
jq -e --arg d "$dock" 'length == 2 and all(.[0] | startswith($d + "@pci-"))' <<<"$both" >/dev/null \
    || fail "not kept apart by port: $both"
"$root/scripts/vm.sh" removable-detach dock0
docked=(dock1)
vm "sudo systemctl restart zendiloosd"
wait_for "zendiloosd to answer again" "[[ \$(status GET /storage/inventory) == 200 ]]" 90
wait_for "one disk of the dock" "[[ \$(records | jq 'map(select(.[1] != null)) | length') == 1 ]]" 30
alone=$(records)
[[ $(jq -c 'map(.[0])' <<<"$alone") == "$(jq -c 'map(.[0])' <<<"$both")" ]] || fail "other records: $both, then $alone"
"$root/scripts/vm.sh" removable-detach dock1
docked=()
wait_for "the dock's disks to go" "[[ \$(records | jq 'map(select(.[1] != null)) | length') == 0 ]]" 30
for record in $(jq -r '.[][0]' <<<"$both"); do
    [[ $(status DELETE "$(record_path "$record")") == 204 ]] || fail "forgetting $record: $(cat "$work/last")"
done

say "Putting the enclosure back"
choice=$(as_choice <"$work/enclosure")
if [[ $choice == null ]]; then
    [[ $(status DELETE /storage/enclosure) == 200 ]] || fail "reset: $(cat "$work/last")"
else
    [[ $(status PUT /storage/enclosure "$choice") == 200 ]] || fail "back: $(cat "$work/last")"
fi
[[ $(call GET /storage/enclosure | as_choice) == "$choice" ]] || fail "not as it was"

say "All enclosure tests passed"
