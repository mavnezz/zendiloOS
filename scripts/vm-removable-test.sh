#!/usr/bin/env bash
# Tests removable drives end to end in a test VM, through the HTTPS API,
# with drives plugged in and pulled out through QEMU's monitor:
#
#   1. a FAT, an exFAT, an NTFS and an ext4 drive are mounted when seen:
#      under /run/zendiloos-removable, nosuid, nodev, noexec; FAT for the
#      users' group, nothing runs from it
#   2. a write-protected drive is mounted read only; the explorer works on
#      the drives: uploads (one larger than 1 MiB, through nginx, back whole
#      as a download and in an archive), a file of a foreign UID on ext4,
#      copies into a share that take the owner of the folder they land in,
#      the group zendiloos-files and the share's ACL, a move out of a share,
#      deletion; a user who is no administrator
#      is refused; thousands of files moved off FAT and exFAT while the
#      kernel keeps dropping its caches (so it numbers their entries anew)
#      arrive whole and leave nothing; a move tried again into a copy partly
#      there leaves what differs and replaces nothing; one whose source is
#      renamed meanwhile is not done; a folder downloaded while a file in it
#      grows ends short, and the files agent says why
#   3. a drive with a file open is not ejected; then it is, and is gone; one
#      ejected without powering off stays so when zendiloosd and the agent
#      restart, until it is plugged in again
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
share=removabletest

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

job_succeeded() {
    local id=$1 state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 120
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

# How job $1 ended, within $2 seconds: its state, code and detail.
job_state() {
    local id=$1
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "${2:-120}"
    call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")'
}

# Moves $2 from the drive $1 into the share, as a job; its id.
move_off() {
    call POST "/files/removable/$1/transfer" \
        "{\"mode\":\"move\",\"paths\":[\"$2\"],\"target_share\":$share_id,\"target_path\":\"\"}" | jq -r .id
}

# $2 files of a few KiB in ten folders, and four of 1 MiB, in $1 in the guest.
fill() {
    vm "sudo sh -c 'set -e; for d in \$(seq 1 10); do mkdir -p \"$1/d\$d\"; for f in \$(seq 1 $(($2 / 10))); do
            head -c \$((512 + f * 7)) /dev/urandom >\"$1/d\$d/f\$f.bin\"; done; done
        for n in 1 2 3 4; do head -c 1048576 /dev/urandom >\"$1/big\$n.bin\"; done'"
}

# The files in $1 in the guest, with their SHA-256, by path.
sums() { vm "sudo sh -c 'cd \"$1\" && find . -type f -print0 | sort -z | xargs -0 sha256sum'"; }

# Runs the script on stdin in the guest as root, in the background, as $1.
background=()
in_background() {
    vm "cat >/tmp/zdl-$1.sh"
    vm "sudo sh -c 'nohup sh /tmp/zdl-$1.sh </dev/null >/dev/null 2>&1 & echo \$! >/tmp/zdl-$1.pid'"
    background+=("$1")
}
stop_background() {
    vm "sudo sh -c 'kill \$(cat /tmp/zdl-$1.pid 2>/dev/null) 2>/dev/null; rm -f /tmp/zdl-$1.pid /tmp/zdl-$1.sh'"
}
# The status code of a raw upload of $3 as $2 into $1 (an API path).
upload() {
    curl -sk -o "$work/last" -w '%{http_code}' -b "$work/admin.jar" -X POST -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/admin.csrf")" -H 'Content-Type: application/octet-stream' \
        --data-binary "@$3" "$api$1/upload?name=$2"
}

delete_share() {
    local id
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
    done
    id=$(call GET /accounts | jq -r '.[] | select(.name == "rmuser") | .id')
    [[ -z $id ]] || call DELETE "/accounts/$id" >/dev/null
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
    for name in "${background[@]}"; do stop_background "$name"; done
    delete_share
    for id in "${plugged[@]}"; do "$root/scripts/vm.sh" removable-detach "$id" 2>/dev/null; done
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"

say "Cleaning up an earlier run"
delete_share

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

say "2b. Files on the drives, and into and out of a share"
pool=$(call GET /storage/pools | jq -er '[.[] | select(.mounted != false)][0].name') || fail "no pool"
volume=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .volume')
me=$(call GET /auth/session | jq -er .account.id)
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,shared:false,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
shared="/volume$volume/$share"
fat_id=$(drive ZFAT | jq -er .id) ext4_id=$(drive ZEXT4 | jq -er .id)
fat=$(drive ZFAT | jq -er .path) ext4=$(drive ZEXT4 | jq -er .path)
echo 'up on the stick' >"$work/up.txt"
[[ $(upload "/files/removable/$fat_id" up.txt "$work/up.txt") =~ ^20 ]] || fail "upload: $(cat "$work/last")"
[[ $(vm "sudo stat -c '%U:%G' '$fat/up.txt'") == root:zendiloos-users ]] || fail "the upload's owner on FAT"
call GET "/files/removable/$fat_id/folder" | jq -e '.entries | map(.name) | index("up.txt")' >/dev/null \
    || fail "the listing: $(call GET "/files/removable/$fat_id/folder")"
[[ $(call GET "/files/removable/$fat_id/download?path=up.txt") == 'up on the stick' ]] || fail "download from FAT"
# More than nginx's default limit for a request's body (1 MiB): passed on as
# it arrives, as into a share, and back whole, also in an archive.
head -c $((3 * 1024 * 1024)) /dev/urandom >"$work/big.bin"
[[ $(upload "/files/removable/$fat_id" big.bin "$work/big.bin") =~ ^20 ]] || fail "a large upload to the stick: $(cat "$work/last")"
call GET "/files/removable/$fat_id/download?path=big.bin" >"$work/big.back"
cmp -s "$work/big.bin" "$work/big.back" || fail "the large file did not come back whole ($(stat -c %s "$work/big.back") bytes)"
call GET "/files/removable/$fat_id/archive?path=" >"$work/stick.tar"
tar -tf "$work/stick.tar" >"$work/stick.list" || fail "the stick's archive cannot be read"
grep -q '/big\.bin$' "$work/stick.list" || fail "the large file is not in the archive: $(cat "$work/stick.list")"
# Formatted elsewhere: owned by someone this NAS does not know, for them alone.
vm "echo secret | sudo tee '$ext4/foreign.txt' >/dev/null && sudo chown 4321:4321 '$ext4/foreign.txt' && sudo chmod 600 '$ext4/foreign.txt'"
[[ $(call GET "/files/removable/$ext4_id/download?path=foreign.txt") == secret ]] || fail "ext4's foreign file"
call GET "/files/removable/$ext4_id/search?path=&text=FOREIGN" | jq -e '[.entries[].path] | index("foreign.txt")' >/dev/null \
    || fail "the search on the drive: $(call GET "/files/removable/$ext4_id/search?path=&text=FOREIGN")"
# Into the share: the owner of the folder it lands in and the group
# zendiloos-files, with the share's ACL, as an upload.
job_succeeded "$(call POST "/files/removable/$fat_id/transfer" \
    "{\"mode\":\"copy\",\"paths\":[\"up.txt\"],\"target_share\":$share_id,\"target_path\":\"\"}" | jq -r .id)"
job_succeeded "$(call POST "/files/removable/$ext4_id/transfer" \
    "{\"mode\":\"copy\",\"paths\":[\"foreign.txt\"],\"target_share\":$share_id,\"target_path\":\"\"}" | jq -r .id)"
for name in up.txt foreign.txt; do
    [[ $(vm "sudo stat -c %U:%G '$shared/$name'") == root:zendiloos-files ]] \
        || fail "$name in the share is $(vm "sudo stat -c %U:%G '$shared/$name'")"
    vm "sudo getfacl -p '$shared/$name' | grep -q '^group:zendiloos-files:rw'" || fail "$name lacks the share's ACL"
done
[[ $(vm "sudo cat '$shared/foreign.txt'") == secret ]] || fail "the copied file's content"
vm "sudo test -f '$fat/up.txt'" || fail "a copy took the original"
# Out of the share onto the stick, moved.
echo 'from the share' >"$work/back.txt"
[[ $(upload "/files/shares/$share_id" back.txt "$work/back.txt") =~ ^20 ]] || fail "upload to the share: $(cat "$work/last")"
job_succeeded "$(call POST "/files/shares/$share_id/transfer" \
    "{\"mode\":\"move\",\"paths\":[\"back.txt\"],\"target_drive\":\"$fat_id\",\"target_path\":\"\"}" | jq -r .id)"
[[ $(vm "sudo cat '$fat/back.txt'") == 'from the share' ]] || fail "the move did not reach the stick"
vm "sudo test ! -e '$shared/back.txt'" || fail "the move left the source"
# Deleting on a drive is for good: no bin there.
job_succeeded "$(call POST "/files/removable/$fat_id/delete" '{"paths":["up.txt","back.txt","big.bin"]}' | jq -r .id)"
vm "sudo test ! -e '$fat/up.txt' && sudo test ! -e '$fat/back.txt' && sudo test ! -e '$fat/big.bin' && sudo test ! -e '$fat/.recycle'" || fail "deleting on the stick"
# Someone who is no administrator sees no drives.
call POST /accounts '{"name":"rmuser","role":"user","password":"third secret phrase"}' | jq -er .id >/dev/null || fail "rmuser"
curl -sk -c "$work/rmuser.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d '{"name":"rmuser","password":"third secret phrase"}' "$api/auth/login" | jq -er .csrf_token >"$work/rmuser.csrf"
curl -sk -b "$work/rmuser.jar" -c "$work/rmuser.jar" -X POST -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/rmuser.csrf")" \
    -H 'Content-Type: application/json' -d '{"current_password":"third secret phrase","new_password":"fourth secret phrase"}' \
    "$api/auth/password" >/dev/null
for path in /storage/removable "/files/removable/$fat_id/folder"; do
    code=$(curl -sk -o /dev/null -w '%{http_code}' -b "$work/rmuser.jar" -H "Origin: $base" "$api$path")
    [[ $code == 403 ]] || fail "a user got $code for $path"
done

say "2c. Thousands of files moved off FAT and exFAT while the kernel drops its caches"
exfat_id=$(drive ZEXFAT | jq -er .id) exfat=$(drive ZEXFAT | jq -er .path)
fill "$fat/many-fat" 3000
fill "$exfat/many-exfat" 3000
sums "$fat/many-fat" >"$work/many-fat.sums"
sums "$exfat/many-exfat" >"$work/many-exfat.sums"
# FAT and exFAT number an entry anew whenever the kernel forgot it.
in_background drop <<'EOF'
while :; do echo 2 >/proc/sys/vm/drop_caches; sleep 0.2; done
EOF
for moved in "$fat_id many-fat $fat" "$exfat_id many-exfat $exfat"; do
    read -r id name place <<<"$moved"
    state=$(job_state "$(move_off "$id" "$name")" 300)
    [[ $state == succeeded* ]] || fail "moving $name off the drive: $state"
    vm "sudo test ! -e '$place/$name'" || fail "$name left on the drive"
    sums "$shared/$name" | cmp -s - "$work/$name.sums" || fail "$name did not arrive whole"
    [[ $(wc -l <"$work/$name.sums") == 3004 ]] || fail "$name: $(wc -l <"$work/$name.sums") files"
done
stop_background drop

say "2d. Tried again into a copy partly there: what differs stays, nothing is replaced"
vm "sudo sh -c 'set -e; mkdir -p \"$fat/again/sub\" \"$shared/again\"
    for n in a b c; do echo \"\$n from the stick\" >\"$fat/again/\$n.txt\"; done
    echo d >\"$fat/again/sub/d.txt\"; cp \"$fat/again/a.txt\" \"$shared/again/\"; echo theirs >\"$shared/again/b.txt\"'"
state=$(job_state "$(move_off "$fat_id" again)")
# The file, and the folder that holds it.
[[ $state =~ ^failed\ partly_moved\ 2\ (.*/)?again/b\.txt$ ]] || fail "tried again: $state"
[[ $(vm "sudo cat '$shared/again/b.txt'") == theirs ]] || fail "the target's file was replaced"
[[ $(vm "sudo ls -A '$fat/again'") == b.txt ]] || fail "left on the stick: $(vm "sudo ls -A '$fat/again'")"
[[ $(vm "sudo cat '$shared/again/c.txt' '$shared/again/sub/d.txt'") == $'c from the stick\nd' ]] \
    || fail "what was missing did not come"

say "2e. A move whose source is renamed meanwhile is not done"
fill "$exfat/renamed" 2000
# As soon as the copy begins in the share.
in_background rename <<EOF
n=0
until [ -e '$shared/renamed' ] || [ \$n -ge 3000 ]; do n=\$((n + 1)); sleep 0.02; done
mv '$exfat/renamed' '$exfat/renamed-away'
EOF
state=$(job_state "$(move_off "$exfat_id" renamed)" 300)
vm "sudo test -d '$exfat/renamed-away' && sudo test ! -e '$exfat/renamed'" || fail "not renamed during the move"
[[ $state == "failed partly_moved "* ]] || fail "a move whose source was renamed: $state"
[[ $(vm "sudo find '$exfat/renamed-away' -type f | wc -l") == 2004 ]] || fail "something of the renamed source went"
stop_background rename

say "2f. A folder downloaded while a file in it grows ends short, and says why"
vm "sudo sh -c 'mkdir -p \"$shared/growing\" && head -c 33554432 /dev/urandom >\"$shared/growing/log.bin\"'"
in_background grow <<EOF
n=0
while [ \$n -lt 3000 ]; do head -c 4096 /dev/zero >>'$shared/growing/log.bin'; n=\$((n + 1)); sleep 0.01; done
EOF
code=0
curl -sk -o "$work/growing.tar" -b "$work/admin.jar" -H "Origin: $base" \
    "$api/files/shares/$share_id/archive?path=growing" || code=$?
stop_background grow
[[ $code == 18 ]] || fail "the archive of a growing file: curl's exit $code, $(stat -c %s "$work/growing.tar") bytes"
vm "sudo journalctl -u zendiloos-files -u zendiloos-agent --since -2min --no-pager" \
    | grep -q 'entries changed while it was written' || fail "the files agent did not say why"
delete_share

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

say "3b. Ejected, it stays so when zendiloosd and the agent restart, until plugged in again"
id=$(drive ZFAT | jq -er .id) place=$(drive ZFAT | jq -er .path) disk=$(drive ZFAT | jq -er .disk)
[[ $(status POST "/storage/removable/$id/eject" '{"power_off":false}') =~ ^20 ]] || fail "eject: $(cat "$work/last")"
drive ZFAT | jq -e '.state == "ejected"' >/dev/null || fail "not ejected: $(drive ZFAT)"
vm "sudo systemctl restart zendiloos-agent zendiloosd"
wait_for "zendiloosd to answer again" "[[ \$(status GET /storage/removable) == 200 ]]" 90
# A look, as the list is one, mounts nothing ejected.
drive ZFAT | jq -e '.state == "ejected"' >/dev/null || fail "after the restarts: $(drive ZFAT)"
vm "! findmnt '$place' >/dev/null" || fail "mounted again after the restarts"
# Pulled out and plugged in again, not necessarily while anyone looked.
pull zfat
wait_for "the disk to go" "! vm 'lsblk -dno NAME' | grep -qx '$disk'" 30
plug "$work/fat.img" zfatagain
wait_for "ZFAT to be mounted again" "mounted ZFAT" 90

say "Back as before"
for label in ZFAT ZEXT4 ZREADONLY; do
    id=$(drive "$label" | jq -er .id)
    [[ $(status POST "/storage/removable/$id/eject" '{"power_off":true}') =~ ^20 ]] || fail "eject $label: $(cat "$work/last")"
done
pull zfatagain; pull zext4; pull zro

say "All removable drive tests passed"
