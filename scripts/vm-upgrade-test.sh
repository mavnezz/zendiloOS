#!/usr/bin/env bash
# Upgrade test (docs/roadmap.md, M5): a machine set up with an older
# package keeps its accounts, pool, share, files and passwords when a newer
# package is installed over it, and works afterwards.
#
#   1. installs OLD on a fresh VM and sets it up: an administrator, a mirror
#      pool, an account with a share, files written over SMB and uploaded
#      through the file manager
#   2. installs NEW over it
#   3. checks that all of it is still there and works: sign-in with the old
#      passwords (web and SMB), the files with their checksums and owners,
#      the share given share-level permissions once, by a job
#      sharing.migrate_permissions of its own (its folder
#      root:zendiloos-files, mode 2770 and the static ACL; the group's
#      entry on the files, nobody's own), a copy of the database from
#      before the migration, and the endpoints the new version added
#
# Usage:
#   just deb-at 142ab01                       # the old package, here M4
#   just deb                                  # the new one
#   ZENDILOOS_VM=2 just vm create && ZENDILOOS_VM=2 just vm start
#   ZENDILOOS_VM=2 scripts/vm-upgrade-test.sh dist/142ab01/zendiloos_0.1.0_amd64.deb
#
# NEW defaults to the newest dist/zendiloos_*.deb. Needs a VM with nothing
# installed; ZENDILOOS_VM picks it, so the usual one can stay as it is.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
old=${1:?usage: $0 OLD.deb [NEW.deb]}
new=${2:-$(ls -t "$root"/dist/zendiloos_*_amd64.deb 2>/dev/null | head -1)}
[[ -f $old && -f $new ]] || { echo "packages not found: $old $new" >&2; exit 1; }
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
admin_pw='correct horse battery'
anna_pw='fresh secret phrase one'
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call_as() {
    local session=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk -b "$work/$session.jar" -c "$work/$session.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/$session.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}
call() { call_as admin "$@"; }
status() { curl -sk -o /dev/null -w '%{http_code}' -b "$work/admin.jar" "$api$1"; }

sign_in() { # session name password
    curl -sk -c "$work/$1.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$2" --arg password "$3" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/$1.csrf" || fail "cannot sign in as $2"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

job_succeeded() {
    local id=$1 state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

smb() { vm "cd /tmp && smbclient //localhost/photos -U 'anna%$anna_pw' -c '$*' 2>&1" || true; }

# Everything that must survive, as text to compare before and after: the
# files' contents and owners. The share's own folder and the files' ACLs
# change with the update (checked on their own after it).
snapshot() {
    # The share is closed to the VM's own user: everything as root.
    vm "sudo sh -c 'cd $path && sha256sum from-smb.bin album/uploaded.txt && stat -c \"%n %U:%G\" from-smb.bin album album/uploaded.txt'" 2>&1
}

# The newest job sharing.migrate_permissions of share $1, if there is one.
migration() { call GET '/jobs?limit=200' | jq -c --arg share "$1" \
    '[.[] | select(.kind == "sharing.migrate_permissions" and .target == $share)][0] // empty'; }

say "0. A fresh VM"
vm "! dpkg -s zendiloos >/dev/null 2>&1" || fail "zendiloos is installed; this needs a fresh VM"
vm "command -v smbclient >/dev/null || { sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient >/dev/null; }"

say "1. The old version, set up and in use: $(basename "$old")"
"$root/scripts/vm.sh" install "$old"
# Packages up to M4 left the freshly built ZFS module unloaded until
# the next boot (see debian/zendiloos.postinst).
vm "test -d /sys/module/zfs || sudo modprobe zfs"
wait_for "the API" "curl -sfk $api/health"
code=$(vm 'sudo cat /var/lib/zendiloos/setup-code')
curl -sfk -o /dev/null -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg code "$code" --arg password "$admin_pw" '{setup_code: $code, name: "admin", password: $password}')" \
    "$api/setup" || fail "setup"
sign_in admin admin "$admin_pw"
free=$(call GET /storage/disks | jq -c '[.[] | select(.usage == "unused" and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
token=$(call POST /storage/pools/plan "{\"name\":\"tank\",\"layout\":\"mirror\",\"disks\":$free}" | jq -er .token) || fail "plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"tank\"}" | jq -r .id)"
anna=$(call POST /accounts '{"name":"anna","role":"user","password":"first secret phrase"}' | jq -er .id) || fail "anna"
sign_in anna anna 'first secret phrase'
call_as anna POST /auth/password "{\"current_password\":\"first secret phrase\",\"new_password\":\"$anna_pw\"}"
share=$(call POST /shares "{\"name\":\"photos\",\"description\":\"Family photos\",\"pool\":\"tank\",\"access\":[{\"kind\":\"account\",\"id\":$anna,\"access\":\"read_write\"}],\"nfs\":[]}")
share_id=$(jq -er .id <<<"$share") || fail "share: $share"
path=$(jq -r .path <<<"$share")
vm "head -c 1048576 /dev/urandom > /tmp/from-smb.bin"
out=$(smb 'put from-smb.bin; mkdir album'); ! grep -q NT_STATUS <<<"$out" || fail "SMB: $out"
sign_in anna anna "$anna_pw"
echo "uploaded before the upgrade" > "$work/uploaded.txt"
curl -sfk -o /dev/null -b "$work/anna.jar" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/anna.csrf")" \
    -H 'Content-Type: application/octet-stream' --data-binary @"$work/uploaded.txt" \
    "$api/files/shares/$share_id/upload?path=album&name=uploaded.txt" || fail "upload"
snapshot >"$work/before"
schema_before=$(vm "sudo ls /var/lib/zendiloos/backup 2>/dev/null | wc -l")

say "2. The new version over it: $(basename "$new")"
"$root/scripts/vm.sh" install "$new"
wait_for "the API" "curl -sfk $api/health"
vm "systemctl is-active --quiet zendiloosd" || fail "zendiloosd does not run"

say "3. Everything is still there and works"
sign_in admin admin "$admin_pw"
call GET /accounts | jq -e '.[] | select(.name == "anna")' >/dev/null || fail "anna is gone"
call GET /shares | jq -e --arg path "$path" '.[] | select(.name == "photos" and .path == $path)' >/dev/null \
    || fail "the share is gone"
call GET /storage/pools | jq -e '.[] | select(.name == "tank" and .state == "online" and .mount_point == "/volume1")' \
    >/dev/null || fail "the pool is not online at /volume1"
# Shares made before 0.3.0 are walked to share-level access once, one after
# the other, as zendiloosd starts; later ones have it from the start.
if dpkg --compare-versions "$(dpkg-deb -f "$old" Version)" lt 0.3.0~; then
    for name in $(call GET /shares | jq -r '.[].name'); do
        wait_for "the migration of $name" "migration $name | jq -e .id" 600
        job_succeeded "$(migration "$name" | jq -r .id)"
    done
fi
snapshot >"$work/after"
diff -u "$work/before" "$work/after" || fail "files or owners changed"
[[ $(vm "sudo stat -c '%U:%G %a' $path") == 'root:zendiloos-files 2770' ]] \
    || fail "the share's folder is $(vm "sudo stat -c '%U:%G %a' $path")"
acl=$(vm "sudo getfacl -cpE $path")
for entry in group:zendiloos-files:rwx default:group:zendiloos-files:rwx mask::rwx; do
    grep -qx "$entry" <<<"$acl" || fail "the share's folder lacks $entry: $acl"
done
named=$(grep -E '^(default:)?(user|group):[^:]+:' <<<"$acl" | grep -vE '^(default:)?group:zendiloos-files:' || true)
[[ -z $named ]] || fail "the share's folder still names $(tr '\n' ' ' <<<"$named")"
for entry in from-smb.bin album album/uploaded.txt; do
    acl=$(vm "sudo getfacl -cpE '$path/$entry'")
    grep -q '^group:zendiloos-files:rw' <<<"$acl" || fail "$entry lacks the group's entry: $acl"
    ! grep -qE '^(default:)?user:anna:' <<<"$acl" || fail "$entry still names anna: $acl"
done
out=$(smb 'get from-smb.bin /tmp/back.bin'); ! grep -q NT_STATUS <<<"$out" || fail "SMB with the old password: $out"
vm "cmp -s /tmp/from-smb.bin /tmp/back.bin" || fail "the file read over SMB differs"
sign_in anna anna "$anna_pw"
call_as anna GET "/files/shares/$share_id/folder?path=album" | jq -e '.entries[] | select(.name == "uploaded.txt")' \
    >/dev/null || fail "the file manager does not show the upload"
backups=$(vm "sudo ls /var/lib/zendiloos/backup | wc -l")
(( backups > schema_before )) || fail "no copy of the database was made before migrating"
vm "sudo ls -l /var/lib/zendiloos/backup" | sed 's/^/   /'
for endpoint in /storage/overview /network /preferences /system/metrics/history; do
    [[ $(status "$endpoint") == 200 ]] || fail "$endpoint does not answer"
done
call GET /notifications | jq -e 'has("read_up_to")' >/dev/null || fail "notifications in the old form"
vm "testparm -s 2>/dev/null | grep -q 'comment = Family photos'" || fail "the share's description is not its comment"

say "Passed: accounts, pool, share, files and passwords survive the upgrade, and the share has share-level permissions"
