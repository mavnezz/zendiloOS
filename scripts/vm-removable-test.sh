#!/usr/bin/env bash
# Tests removable drives end to end in a test VM, through the HTTPS API,
# with drives plugged in and pulled out through QEMU's monitor:
#
#   1. a FAT, an exFAT, an NTFS and an ext4 drive are mounted when seen:
#      under /run/zendiloos-removable, nosuid, nodev, noexec; FAT for the
#      users' group, nothing runs from it
#   2. a write-protected drive is mounted read only
#   3. a drive with a file open is not ejected; then it is, and is gone
#   4. a drive pulled out unannounced is cleaned up and reported
#   5. a disk that was part of a pool is never offered
#
# The cloud image's kernel has no USB, so the drives come over SCSI with the
# removable bit (scripts/vm.sh removable-attach). Needs the VM started by a
# vm.sh with the monitor (restart it once if 'removable-attach' says so),
# the package installed and set up; account and password as for
# vm-storage-test.sh. Runs in the second VM by default (the first holds a LAN
# test). Puts everything back.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
plugged=()

say() { printf '\033[1m%s\033[0m\n' "$*"; }
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
    local what=$1 check=$2 seconds=${3:-60}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

plug() { "$root/scripts/vm.sh" removable-attach "$@"; plugged+=("$2"); }
pull() {
    "$root/scripts/vm.sh" removable-detach "$1"
    local left=() id
    for id in "${plugged[@]}"; do [[ $id == "$1" ]] || left+=("$id"); done
    plugged=("${left[@]}")
}

# The drive labelled $1 as the API lists it.
drive() { call GET /storage/removable | jq -ec --arg l "$1" '.[] | select(.label == $l)'; }
mounted() { drive "$1" | jq -e '.state == "mounted"'; }

cleanup() {
    set +e
    for id in "${plugged[@]}"; do "$root/scripts/vm.sh" removable-detach "$id" 2>/dev/null; done
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"

say "Images"
truncate -s 64M "$work/fat.img" "$work/exfat.img" "$work/ro.img"
truncate -s 128M "$work/ntfs.img" "$work/ext4.img"
/usr/sbin/mkfs.vfat -n ZFAT "$work/fat.img" >/dev/null
/usr/sbin/mkfs.vfat -n ZREADONLY "$work/ro.img" >/dev/null
/usr/sbin/mkfs.exfat -L ZEXFAT "$work/exfat.img" >/dev/null
/usr/sbin/mkfs.ntfs -q -F -f -L ZNTFS "$work/ntfs.img" >/dev/null
mkdir "$work/ext4-content" && echo 'from elsewhere' >"$work/ext4-content/note.txt"
/usr/sbin/mkfs.ext4 -q -L ZEXT4 -d "$work/ext4-content" "$work/ext4.img"

say "1. Mounted when seen"
plug "$work/fat.img" zfat
plug "$work/exfat.img" zexfat
plug "$work/ntfs.img" zntfs
plug "$work/ext4.img" zext4
for label in ZFAT ZEXFAT ZNTFS ZEXT4; do
    wait_for "$label to be mounted" "mounted $label" 90
    place=$(drive "$label" | jq -er .path)
    [[ $place == /run/zendiloos-removable/* ]] || fail "$label at $place"
    options=$(vm "findmnt -no OPTIONS '$place'")
    for option in nosuid nodev noexec; do
        [[ ,$options, == *,$option,* ]] || fail "$label lacks $option: $options"
    done
done
[[ $(drive ZFAT | jq -r .filesystem) == fat && $(drive ZNTFS | jq -r .filesystem) == ntfs \
    && $(drive ZEXFAT | jq -r .filesystem) == exfat && $(drive ZEXT4 | jq -r .filesystem) == ext4 ]] \
    || fail "file systems: $(call GET /storage/removable | jq -c 'map({label, filesystem})')"
fat=$(drive ZFAT | jq -er .path)
[[ $(vm "stat -c '%U:%G' '$fat'") == root:zendiloos-users ]] || fail "FAT's owner: $(vm "stat -c '%U:%G' '$fat'")"
[[ $(vm "sudo stat -c %a '$fat'") == 770 ]] || fail "FAT's mode: $(vm "sudo stat -c %a '$fat'")"
# Nothing runs from it, not even as root.
vm "printf '#!/bin/sh\\necho ran\\n' | sudo tee '$fat/run.sh' >/dev/null && sudo chmod +x '$fat/run.sh' 2>/dev/null; ! sudo '$fat/run.sh' 2>/dev/null" \
    || fail "a script ran from the drive"
[[ $(vm "sudo cat '$(drive ZEXT4 | jq -er .path)/note.txt'") == 'from elsewhere' ]] || fail "ext4's file"
drive ZFAT | jq -e '.used_bytes != null and .available_bytes > 0 and .read_only == false' >/dev/null \
    || fail "FAT's figures: $(drive ZFAT)"

say "2. Write-protected: read only"
plug "$work/ro.img" zro ro
wait_for "the protected drive" "mounted ZREADONLY" 90
drive ZREADONLY | jq -e '.read_only == true' >/dev/null || fail "not read only: $(drive ZREADONLY)"
[[ ,$(vm "findmnt -no OPTIONS '$(drive ZREADONLY | jq -er .path)'"), == *,ro,* ]] || fail "mounted writable"

say "3. Eject: not while in use, then gone"
id=$(drive ZEXFAT | jq -er .id) place=$(drive ZEXFAT | jq -er .path)
disk=$(drive ZEXFAT | jq -er .disk)
vm "sudo touch '$place/open.txt'; sudo nohup sh -c 'exec 3<\"$place/open.txt\"; sleep 25' >/dev/null 2>&1 &"
sleep 1
[[ $(status POST "/storage/removable/$id/eject" '{"power_off":true}') == 409 ]] || fail "ejected while in use: $(cat "$work/last")"
jq -e '.code == "drive_in_use"' "$work/last" >/dev/null || fail "$(cat "$work/last")"
mounted ZEXFAT >/dev/null || fail "a refused eject unmounted it"
wait_for "the file to be closed" "! vm \"sudo fuser -m '$place' 2>/dev/null | grep -q '[0-9]'\"" 40
[[ $(status POST "/storage/removable/$id/eject" '{"power_off":true}') =~ ^20 ]] || fail "eject: $(cat "$work/last")"
wait_for "the disk to go" "! vm 'lsblk -dno NAME' | grep -qx '$disk'" 30
vm "! findmnt '$place' >/dev/null && test ! -e '$place'" || fail "the place is left behind"
pull zexfat

say "4. Pulled out unannounced"
place=$(drive ZNTFS | jq -er .path)
before=$(call GET /notifications | jq '[.notifications[] | select(.code == "removable_pulled")] | length')
pull zntfs
wait_for "the drive to leave the list" "! drive ZNTFS" 90
wait_for "its place to be cleaned up" "vm \"! findmnt '$place' >/dev/null && test ! -e '$place'\"" 90
wait_for "the notification" "[[ \$(call GET /notifications | jq '[.notifications[] | select(.code == \"removable_pulled\")] | length') -gt $before ]]" 90

say "5. A disk of a pool is never offered"
vm "truncate -s 128M /tmp/zpool.img && sudo zpool create -f zremovabletest /tmp/zpool.img && sudo zpool export zremovabletest"
"$root/scripts/vm.sh" ssh 'cat /tmp/zpool.img' >"$work/zpool.img"
vm "rm -f /tmp/zpool.img"
plug "$work/zpool.img" zzfs
wait_for "the guest to see the disk" "vm \"lsblk -rno NAME,LABEL | grep -q ' zremovabletest\$'\"" 30
name=$(vm "lsblk -rno NAME,LABEL | awk '\$2 == \"zremovabletest\" { print \$1 }'" | head -1)
disk=$(vm "lsblk -dno PKNAME /dev/$name"); disk=${disk:-$name}
[[ -n $disk ]] || fail "the guest does not see the pool's disk"
sleep 3
call GET /storage/removable | jq -e --arg d "$disk" 'map(select(.disk == $d)) | length == 0' >/dev/null \
    || fail "a pool's disk is offered: $(call GET /storage/removable)"
vm "! lsblk -rno NAME,MOUNTPOINTS /dev/$disk | grep -q zendiloos-removable" || fail "a pool's disk got mounted"
pull zzfs

say "Back as before"
for label in ZFAT ZEXT4 ZREADONLY; do
    id=$(drive "$label" | jq -er .id)
    [[ $(status POST "/storage/removable/$id/eject" '{"power_off":true}') =~ ^20 ]] || fail "eject $label: $(cat "$work/last")"
done
pull zfat; pull zext4; pull zro

say "All removable drive tests passed"
