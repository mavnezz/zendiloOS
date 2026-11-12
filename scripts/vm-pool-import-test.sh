#!/usr/bin/env bash
# Tests what follows a configuration backup restored on a machine installed
# afresh with the old machine's data disks (docs/architecture.md, 4.15),
# end to end through the HTTPS API, with zpool, findmnt, systemctl,
# smbclient and nft inside the VM:
#
#   On the third VM, set up afresh:
#   1. a mirror pool on two of its spare disks, a share on it with a file
#      written over SMB, and the firewall on with rules; then a
#      configuration backup downloaded
#   2. the VM reinstalled (`vm.sh reinstall`: a fresh system disk, a new
#      host id, the spare disks as they were) and the package installed:
#      `zpool import` finds the pool, last used by another machine (the old
#      host id), and nothing mounts it
#   3. the backup restored instead of the first setup: the pool is offered
#      on the Storage page, as the restored host id makes it this
#      machine's own (`other_host` false), and nothing is imported by itself
#   4. imported: the volume's and the share's mount units written and
#      started, enabled for the next boot; the file reachable over SMB again
#   5. the network's settings set aside, not told as drift; the firewall's
#      defaults (off) in force, the backup's rules offered; applied, they
#      go back unless confirmed; applied again and confirmed, they hold
#   6. after a reboot the pool is imported and mounted by itself
#
# One VM serves as both machines, since `vm.sh reinstall` keeps the spare
# disks: a fresh third VM (QEMU cannot hand a disk image to another VM
# while the first runs, and the second keeps its own pools). Needs it
# created, started and installed, never set up:
#   ZENDILOOS_VM=3 just vm create && ZENDILOOS_VM=3 just vm start && ZENDILOOS_VM=3 just vm install
#   just vm-pool-import-test
#   ZENDILOOS_VM=3 just vm destroy
# The package it installs again after the reinstall is the newest in dist/
# (or ZENDILOOS_DEB). The VM keeps what the test made; destroy it after.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
vm_number=${ZENDILOOS_VM:-3}
[[ $vm_number != "1" && $vm_number != "2" ]] || { echo "only on a throwaway VM (3 or more): it is reinstalled" >&2; exit 1; }
export ZENDILOOS_VM=$vm_number
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
passphrase="vm test passphrase $(date +%s)"
pool=tank
share=photos
content="written before the reinstall, $(date +%s)"
base="https://127.0.0.1:$((8442 + vm_number))"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
check() { printf '   ✓ %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# METHOD PATH [BODY]: as the administrator signed in.
call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk --max-time 60 -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$base/api/v1$path"
}

# METHOD PATH [BODY]: the HTTP status only, the body in $work/last.
status() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk --max-time 60 -o "$work/last" -w '%{http_code}' -b "$work/jar" -c "$work/jar" -X "$method"
                -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$base/api/v1$path"
}

sign_in() {
    curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$base/api/v1/auth/login" | jq -er .csrf_token >"$work/csrf"
}

wait_for() {
    local what=$1 test=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$test" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The job's end state, once it ended.
job_end() {
    local id=$1 state
    for _ in $(seq 1 600); do
        state=$(call GET "/jobs/$id" | jq -r .state)
        [[ $state == running ]] || { call GET "/jobs/$id" | jq -r '"\(.state) \(.error_code // "") \(.error_detail // "")"'; return; }
        sleep 1
    done
    fail "job $id did not end"
}

setup_code() { vm 'sudo cat /var/lib/zendiloos/setup-code 2>/dev/null || true'; }

# The host reaches the web interface and SSH through QEMU's network.
host_rules='[{"enabled":true,"service":"web_interface","source":"10.0.2.0/24","action":"allow"},
             {"enabled":true,"service":"ssh","source":"10.0.2.0/24","action":"allow"}]'

smb() { vm "cd /tmp && smbclient //localhost/$share -U '$user%$password' -m SMB3 -c '$*' 2>&1"; }
# Whether the file went into the share over SMB.
smb_put() { local out; out=$(smb 'put /tmp/hello.txt hello.txt' || true); [[ $out != *NT_STATUS* ]]; }
# Whether PLACE is a ZFS dataset mounted.
zfs_at() { [[ $(vm "findmnt -n -o FSTYPE $1" 2>/dev/null || true) == "zfs" ]]; }
# Whether the backup's firewall rules are out of force.
rules_gone() { local out; out=$(vm 'sudo nft list table inet zendiloos_firewall' 2>&1 || true); [[ $out != *10.99.0.0/16* ]]; }

vm 'dpkg -s zendiloos >/dev/null 2>&1' || fail "zendiloos is not installed: ZENDILOOS_VM=$vm_number just vm install"
curl -sk "$base/api/v1/setup" | jq -e .required >/dev/null \
    || fail "VM $vm_number is set up already: make it afresh (destroy, create, start, install)"
# The client inside the VM, again after the reinstall, whose fresh system
# disk has none.
smb_client() { vm "command -v smbclient >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient >/dev/null"; }
smb_client

say "1. A pool, a share with a file and the firewall's rules, backed up"
reply=$(curl -sk -o "$work/setup" -w '%{http_code}' -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg n "$user" --arg p "$password" --arg c "$(setup_code)" \
        '{name: $n, password: $p, setup_code: (if $c == "" then null else $c end)}')" "$base/api/v1/setup")
[[ $reply == "2"* ]] || fail "the first setup: $reply $(cat "$work/setup")"
sign_in || fail "cannot sign in"
free=$(call GET /storage/disks | jq -c '[.[] | select(.usage == "unused" and .id_link != null and .transport == "sata") | .name][:2]')
[[ $(jq length <<<"$free") == "2" ]] || fail "two free SATA disks are needed: $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
id=$(call POST /storage/pools "{\"token\":\"$(jq -er .token <<<"$plan")\",\"confirm\":\"$pool\"}" | jq -er .id) \
    || fail "no pool: $plan"
state=$(job_end "$id")
[[ $state == "succeeded"* ]] || fail "creating $pool: $state"
volume=$(jq -er .volume <<<"$plan")
me=$(call GET /auth/session | jq -er .account.id)
call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name: $n, pool: $pool, access: [{kind: "account", id: $me, access: "read_write"}]}')" | jq -e .id >/dev/null \
    || fail "no share"
vm "printf '%s\n' '$content' > /tmp/hello.txt"
wait_for "the share over SMB" smb_put 60
smb 'get hello.txt /tmp/back.txt' >/dev/null
[[ $(vm 'cat /tmp/back.txt') == "$content" ]] || fail "the file did not go in"
firewall=$(jq -cn --argjson host "$host_rules" \
    '{enabled: true, fallback: "allow", rules: ($host + [{enabled: true, service: "ping", source: "10.99.0.0/16", action: "deny"}])}')
[[ $(status PUT /security/firewall "$firewall") == "202" ]] || fail "the firewall: $(cat "$work/last")"
[[ $(status POST /security/firewall/confirm) == "204" ]] || fail "confirming the firewall"
curl -skf -b "$work/jar" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf")" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg p "$passphrase" '{passphrase: $p}')" -o "$work/backup.tar.zst.age" \
    "$base/api/v1/configuration/backup/download" || fail "no backup"
old_hostid=$(vm 'od -An -tx4 /etc/hostid | tr -d " "')
check "$pool (volume $volume) on $(jq -r 'join(", ")' <<<"$free"), $share with hello.txt, the firewall on, host id $old_hostid"

say "2. Reinstalled, the spare disks kept"
"$root/scripts/vm.sh" reinstall
"$root/scripts/vm.sh" start
"$root/scripts/vm.sh" install "${ZENDILOOS_DEB:-}"
wait_for "zendiloosd" "curl -skf $base/api/v1/health" 180
smb_client
new_hostid=$(vm 'od -An -tx4 /etc/hostid 2>/dev/null | tr -d " "')
[[ $new_hostid != "$old_hostid" ]] || fail "the reinstalled machine has the old host id"
listed=$(vm 'sudo zpool import -d /dev/disk/by-id 2>&1' || true)
grep -q "pool: $pool" <<<"$listed" || fail "zpool import does not find $pool: $listed"
grep -q 'another system' <<<"$listed" || fail "not told as another machine's: $listed"
mounted=$(vm 'findmnt -n /volume1 || true')
[[ -z $mounted ]] || fail "something is mounted at /volume1: $mounted"
check "zpool import finds $pool, last used by another machine (host id $new_hostid now); nothing mounted"

say "3. The backup restored instead of the first setup"
status=$(curl -sk -o "$work/restored.json" -w '%{http_code}' -X POST -H "Origin: $base" \
    -H 'Content-Type: application/octet-stream' -H "X-Zendiloos-Passphrase: $(printf %s "$passphrase" | base64 -w0)" \
    -H "X-Zendiloos-Setup-Code: $(setup_code)" --data-binary "@$work/backup.tar.zst.age" "$base/api/v1/setup/restore")
[[ $status == "200" ]] || fail "not restored ($status): $(cat "$work/restored.json")"
sleep 5
wait_for "zendiloosd after the restore" "curl -skf $base/api/v1/health" 180
sign_in || fail "the backup's administrator cannot sign in"
[[ $(vm 'od -An -tx4 /etc/hostid | tr -d " "') == "$old_hostid" ]] || fail "the host id is not the backup's"
absent=$(call GET /storage/pools/absent)
entry=$(jq -c --arg p "$pool" '.known[] | select(.pool.name == $p)' <<<"$absent")
[[ -n $entry ]] || fail "$pool is not offered: $absent"
jq -e --argjson v "$volume" --arg s "$share" \
    '.pool.volume == $v and (.pool.shares | index($s)) and (.found.other_host | not) and .found.can_import' <<<"$entry" >/dev/null \
    || fail "offered as: $entry"
mounted=$(vm 'findmnt -n /volume1 || true')
[[ -z $mounted ]] || fail "imported by itself: $mounted"
notified=$(call GET /notifications | jq -c '[.. | objects | select(.code? == "pools_not_imported")] | length')
[[ $notified -ge 1 ]] || fail "the pools not imported are not told"
check "$pool offered as volume $volume with $share, this machine's own by the restored host id; told; not imported by itself"

say "4. Imported"
pool_id=$(jq -r .found.id <<<"$entry")
[[ $(status POST /storage/pools/import "$(jq -n --arg p "$pool" --arg id "$pool_id" '{name: $p, id: $id}')") == "202" ]] \
    || fail "import: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == "succeeded"* ]] || fail "importing: $state"
for place in "/volume$volume" "/volume$volume/$share"; do
    zfs_at "$place" || fail "$place is not mounted: $(vm "findmnt -n $place" || true)"
done
for unit in "volume$volume.mount" "volume$volume-$share.mount"; do
    enabled=$(vm "systemctl is-enabled $unit 2>&1 || true")
    [[ $enabled == "enabled" ]] || fail "$unit is $enabled"
done
smb 'get hello.txt /tmp/back.txt' >/dev/null
[[ $(vm 'cat /tmp/back.txt') == "$content" ]] || fail "hello.txt is not reachable over SMB again"
absent=$(call GET /storage/pools/absent)
jq -e '.known | length == 0' <<<"$absent" >/dev/null || fail "still offered: $absent"
check "/volume$volume and /volume$volume/$share mounted, their units enabled, hello.txt over SMB"

say "5. The network's settings set aside, the firewall's applied with confirmation"
call GET /network/setup | jq -e '.drift == null' >/dev/null || fail "drift told: $(call GET /network/setup | jq -c .drift)"
restored_network=$(call GET /network/restored)
check "no drift; the backup's network: $(jq -c 'if . == null then "none (zendiloOS wrote no files)" else [.interfaces[] | {interface, standing}] end' <<<"$restored_network")"
fw=$(call GET /security/firewall)
jq -e '(.current.enabled | not) and .restored.settings.enabled' <<<"$fw" >/dev/null \
    || fail "the firewall after the restore: $(jq -c '{current, restored}' <<<"$fw")"
table=$(vm 'sudo nft list table inet zendiloos_firewall 2>&1 || true')
grep -q 'No such file\|does not exist' <<<"$table" || fail "rules in force before they were confirmed: $table"
[[ $(status POST /security/firewall/restored/apply '{}') == "202" ]] || fail "apply: $(cat "$work/last")"
jq -e '.pending != null' "$work/last" >/dev/null || fail "not waiting for confirmation"
grep -q '10.99.0.0/16' <<<"$(vm 'sudo nft list table inet zendiloos_firewall')" || fail "the backup's rules are not in force"
within=$(call GET /security/firewall | jq -r .confirm_within)
wait_for "the rules going back" rules_gone $((within + 60))
call GET /security/firewall | jq -e '.restored != null and (.current.enabled | not)' >/dev/null \
    || fail "after going back: $(call GET /security/firewall | jq -c '{current, restored}')"
[[ $(status POST /security/firewall/restored/apply '{}') == "202" ]] || fail "apply again: $(cat "$work/last")"
[[ $(status POST /security/firewall/confirm) == "204" ]] || fail "confirming"
call GET /security/firewall | jq -e '.restored == null and .current.enabled' >/dev/null \
    || fail "after confirming: $(call GET /security/firewall | jq -c '{current, restored}')"
check "unconfirmed the defaults came back; confirmed, the backup's rules hold"

say "6. After a reboot"
vm 'sudo systemctl reboot' || true
sleep 10
wait_for "zendiloosd after the reboot" "curl -skf $base/api/v1/health" 300
wait_for "/volume$volume/$share" "zfs_at /volume$volume/$share" 120
[[ $(vm "sudo cat /volume$volume/$share/hello.txt") == "$content" ]] || fail "hello.txt after the reboot"
check "$pool imported and mounted by itself"

say "Passed: a pool of the old installation imported after a restore, its share reachable; the firewall's rules applied with confirmation"
