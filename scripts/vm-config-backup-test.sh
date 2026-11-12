#!/usr/bin/env bash
# Tests the configuration backup end to end, through the HTTPS API, with
# getent, testparm, smbclient and the database inside the VMs:
#
#   On the second VM (the source):
#   1. an account made for the test (its UID noted, an SMB password set),
#      SSH without root's login and a time zone, so there is something to
#      carry (SSH stays on port 22: the test reaches the VMs through it)
#   2. a backup downloaded with a passphrase: age's format; a passphrase too
#      short refused
#   3. a scheduled backup into a share's folder, run now, the newest two
#      kept of three, written as the file manager writes for the
#      administrator
#   4. restores refused: the wrong passphrase, something not a backup
#   On a fresh third VM (the target, installed but never set up):
#   5. the backup restored instead of the first setup, with the setup code;
#      zendiloosd starts again with it
#   6. the source's administrator signs in; the test account is there with
#      its UID and its SMB password (smbclient), the source's name, time
#      zone and SSH's rules too; the network stays the target's own
#   7. back on the second VM, a database newer than its release is refused:
#      a backup of the target's after a migration row from the future
#
# Needs both VMs running with the package installed: the second set up
# (account and password as for vm-storage-test.sh) with a pool and a share
# for step 3 (ZENDILOOS_BACKUP_SHARE, the first share otherwise); the third
# fresh, never set up:
#   ZENDILOOS_VM=3 just vm create && ZENDILOOS_VM=3 just vm start && ZENDILOOS_VM=3 just vm install
#   just vm-config-backup-test
#   ZENDILOOS_VM=3 just vm destroy
# The second VM is put back as it was (its account, SSH, time zone, plan);
# the third keeps the restored configuration.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
source_vm=${ZENDILOOS_SOURCE_VM:-2}
target_vm=${ZENDILOOS_TARGET_VM:-3}
[[ $source_vm != 1 && $target_vm != 1 ]] || { echo "not on VM 1: it holds the LAN test" >&2; exit 1; }
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
passphrase="vm test passphrase $(date +%s)"
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
check() { printf '   ✓ %s\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
on() { local vm=$1; shift; ZENDILOOS_VM=$vm "$root/scripts/vm.sh" ssh "$@"; }
base() { echo "https://127.0.0.1:$((8442 + $1))"; }

# call VM METHOD PATH [BODY]: as the administrator signed in on that VM.
call() {
    local vm=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk --max-time 60 -b "$work/$vm.jar" -c "$work/$vm.jar" -X "$method" -H "Origin: $(base "$vm")"
                -H "X-CSRF-Token: $(cat "$work/$vm.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$(base "$vm")/api/v1$path"
}

sign_in() {
    local vm=$1
    curl -sk -c "$work/$vm.jar" -H "Origin: $(base "$vm")" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$(base "$vm")/api/v1/auth/login" | jq -er .csrf_token >"$work/$vm.csrf"
}

wait_for() {
    local what=$1 test=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$test" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# restore VM PATH ARCHIVE PASSPHRASE [SETUP CODE]: the HTTP status, the body in $work/restored.json.
restore() {
    local vm=$1 path=$2 archive=$3 phrase=$4 code=${5:-}
    local args=(-sk -o "$work/restored.json" -w '%{http_code}' -X POST -H "Origin: $(base "$vm")"
                -H 'Content-Type: application/octet-stream'
                -H "X-Zendiloos-Passphrase: $(printf %s "$phrase" | base64 -w0)" --data-binary "@$archive")
    if [[ $path == /configuration/restore ]]; then
        args+=(-b "$work/$vm.jar" -H "X-CSRF-Token: $(cat "$work/$vm.csrf")")
    fi
    [[ -n $code ]] && args+=(-H "X-Zendiloos-Setup-Code: $code")
    curl "${args[@]}" "$(base "$vm")/api/v1$path"
}

ssh_before= zone_before= plan_before=
cleanup() {
    set +e
    if [[ -s $work/$source_vm.csrf ]]; then
        say "Putting the second VM back"
        id=$(call "$source_vm" GET /accounts | jq -r '.[] | select(.name == "cfgtest") | .id')
        [[ -n $id ]] && call "$source_vm" DELETE "/accounts/$id" >/dev/null
        [[ -n $ssh_before ]] && call "$source_vm" PUT /system/ssh "$ssh_before" >/dev/null
        [[ -n $zone_before ]] && call "$source_vm" PUT /system/time "$zone_before" >/dev/null
        [[ -n $plan_before ]] && call "$source_vm" PUT /configuration/backup "$plan_before" >/dev/null
    fi
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in on VM $source_vm"
sign_in "$source_vm" || fail "cannot sign in on VM $source_vm"
on "$target_vm" 'dpkg -s zendiloos >/dev/null 2>&1' \
    || fail "zendiloos is not installed on VM $target_vm: ZENDILOOS_VM=$target_vm just vm install"
curl -sk "$(base "$target_vm")/api/v1/setup" | jq -e .required >/dev/null \
    || fail "VM $target_vm is set up already: make it afresh (ZENDILOOS_VM=$target_vm just vm destroy, create, start, install)"
on "$source_vm" "command -v smbclient >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient >/dev/null"
on "$target_vm" "command -v smbclient >/dev/null && command -v sqlite3 >/dev/null \
    || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient sqlite3 >/dev/null"

say "1. Something to carry on VM $source_vm"
ssh_before=$(call "$source_vm" GET /system/ssh | jq -c '{enabled, port, root_login, password_login}')
zone_before=$(call "$source_vm" GET /system/time | jq -c '{zone, network_time, servers}')
plan_before=$(call "$source_vm" GET /configuration/backup \
    | jq -c '{enabled, frequency, weekday, hour, minute, target, keep}')
secret="cfg test $(date +%s) Secret!"
call "$source_vm" POST /accounts "$(jq -n --arg p "$secret" '{name: "cfgtest", role: "user", password: $p}')" \
    | jq -e .id >/dev/null || fail "no test account"
uid=$(on "$source_vm" 'getent passwd cfgtest' | cut -d: -f3)
[[ $uid =~ ^[0-9]+$ ]] || fail "no Linux user for cfgtest"
call "$source_vm" PUT /system/ssh '{"enabled":true,"port":22,"root_login":"no"}' \
    | jq -e '.root_login == "no"' >/dev/null || fail "SSH's rules not set"
call "$source_vm" PUT /system/time '{"zone":"Europe/Vienna","network_time":true,"servers":[]}' \
    | jq -e '.zone == "Europe/Vienna"' >/dev/null || fail "no time zone"
source_name=$(call "$source_vm" GET /system/hostname | jq -r .name)
check "cfgtest (UID $uid), SSH without root's login, Europe/Vienna, the machine is $source_name"

say "2. A backup downloaded"
status=$(curl -sk -o /dev/null -w '%{http_code}' -b "$work/$source_vm.jar" -H "Origin: $(base "$source_vm")" \
    -H "X-CSRF-Token: $(cat "$work/$source_vm.csrf")" -H 'Content-Type: application/json' \
    -d '{"passphrase":"short"}' "$(base "$source_vm")/api/v1/configuration/backup/download")
[[ $status == 422 ]] || fail "a short passphrase was taken ($status)"
curl -skf -b "$work/$source_vm.jar" -H "Origin: $(base "$source_vm")" -H "X-CSRF-Token: $(cat "$work/$source_vm.csrf")" \
    -H 'Content-Type: application/json' -d "$(jq -n --arg p "$passphrase" '{passphrase: $p}')" \
    -o "$work/backup.tar.zst.age" "$(base "$source_vm")/api/v1/configuration/backup/download" || fail "no backup"
head -c 22 "$work/backup.tar.zst.age" | grep -q 'age-encryption.org/v1' || fail "not age's format"
check "$(du -h "$work/backup.tar.zst.age" | cut -f1), age's format"

say "3. Backups on a schedule into a share, two kept"
share_name=${ZENDILOOS_BACKUP_SHARE:-$(call "$source_vm" GET /shares | jq -r '.[0].name // empty')}
[[ -n $share_name ]] || fail "VM $source_vm has no share for step 3"
share=$(call "$source_vm" GET /shares | jq -r --arg name "$share_name" '.[] | select(.name == $name) | .id')
share_volume=$(call "$source_vm" GET /shares | jq -r --arg name "$share_name" '.[] | select(.name == $name) | .volume')
plan=$(jq -n --argjson share "$share" --arg p "$passphrase" \
    '{enabled: true, frequency: "daily", hour: 3, minute: 0, keep: 2, passphrase: $p,
      target: {kind: "share", share_id: $share, folder: "zendiloOS-test/config"}}')
call "$source_vm" PUT /configuration/backup "$plan" | jq -e '.passphrase_set and .enabled' >/dev/null \
    || fail "no plan: $(call "$source_vm" PUT /configuration/backup "$plan")"
folder=/volume$share_volume/$share_name/zendiloOS-test/config
for run in 1 2 3; do
    job=$(call "$source_vm" POST /configuration/backup/run | jq -er .id) || fail "no run"
    wait_for "run $run" "[[ \$(call $source_vm GET /jobs/$job | jq -r .state) != running ]]" 300
    [[ $(call "$source_vm" GET "/jobs/$job" | jq -r .state) == succeeded ]] \
        || fail "run $run: $(call "$source_vm" GET "/jobs/$job" | jq -c '{error_code, error_detail}')"
    sleep 1
done
count=$(on "$source_vm" "sudo ls $folder | grep -c '^zendiloos-config-.*\\.tar\\.zst\\.age$'")
[[ $count == 2 ]] || fail "$count backups in $folder"
# As the file manager writes: the folder's owner and the group
# zendiloos-files (architecture 4.4), with the administrator's access.
owner=$(on "$source_vm" "sudo stat -c %U:%G $folder/\$(sudo ls $folder | tail -1)")
[[ $owner == "$(on "$source_vm" "sudo stat -c %U $folder"):zendiloos-files" ]] || fail "written as $owner"
call "$source_vm" GET /configuration/backup | jq -e '.last_run.succeeded' >/dev/null || fail "the last run is not noted"
check "two kept in $folder, written as $owner"
on "$source_vm" "sudo rm -rf /volume$share_volume/$share_name/zendiloOS-test"

say "4. Restores refused"
[[ $(restore "$source_vm" /configuration/restore "$work/backup.tar.zst.age" "not the passphrase") == 422 ]] \
    && jq -e '.code == "wrong_passphrase"' "$work/restored.json" >/dev/null || fail "a wrong passphrase: $(cat "$work/restored.json")"
head -c 4096 /dev/urandom >"$work/noise"
[[ $(restore "$source_vm" /configuration/restore "$work/noise" "$passphrase") == 422 ]] \
    && jq -e '.code == "not_a_backup"' "$work/restored.json" >/dev/null || fail "noise: $(cat "$work/restored.json")"
check "a wrong passphrase, noise"

say "5. Restored on VM $target_vm instead of its first setup"
code=$(on "$target_vm" 'sudo cat /var/lib/zendiloos/setup-code 2>/dev/null || true')
status=$(restore "$target_vm" /setup/restore "$work/backup.tar.zst.age" "$passphrase" "$code")
[[ $status == 200 ]] || fail "not restored ($status): $(cat "$work/restored.json")"
check "restored: $(jq -c '{host_name, warnings, network}' "$work/restored.json")"
sleep 5
wait_for "zendiloosd on VM $target_vm" "curl -skf $(base "$target_vm")/api/v1/health" 180
curl -sk "$(base "$target_vm")/api/v1/setup" | jq -e '.required == false' >/dev/null || fail "the setup is still open"

say "6. The source's configuration on VM $target_vm"
sign_in "$target_vm" || fail "the source's administrator cannot sign in"
call "$target_vm" GET /accounts | jq -e '.[] | select(.name == "cfgtest")' >/dev/null || fail "no cfgtest"
[[ $(on "$target_vm" 'getent passwd cfgtest' | cut -d: -f3) == "$uid" ]] || fail "cfgtest has another UID"
on "$target_vm" "sudo pdbedit -L 2>/dev/null | grep -q '^cfgtest:'" || fail "cfgtest has no SMB password"
on "$target_vm" "smbclient -L //127.0.0.1 -U 'cfgtest%$secret' -m SMB3 >/dev/null 2>&1" \
    || fail "cfgtest cannot sign in over SMB with its password"
[[ $(on "$target_vm" hostname) == "$source_name" ]] || fail "the machine is $(on "$target_vm" hostname)"
call "$target_vm" GET /system/time | jq -e '.zone == "Europe/Vienna"' >/dev/null || fail "another time zone"
call "$target_vm" GET /system/ssh | jq -e '.root_login == "no" and .port == 22' >/dev/null \
    || fail "SSH's rules: $(call "$target_vm" GET /system/ssh)"
call "$target_vm" GET /logs/audit | jq -e '[.. | strings | select(test("configuration.restore"))] | length > 0' >/dev/null \
    || fail "the restore is not in the audit log"
on "$target_vm" 'sudo ls /var/lib/zendiloos/backup/ | grep -q before-restore' || fail "the database before is not kept"
check "cfgtest (UID $uid, SMB), $source_name, Europe/Vienna, SSH's rules, audited, the database before kept"

say "7. A database newer than the release refused"
sign_in "$target_vm"
# As zendiloosd's user: a write-ahead log root made would be root's.
on "$target_vm" "sudo -u zendiloos sqlite3 /var/lib/zendiloos/zendiloos.db \"INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time) VALUES (9999, 'from the future', CURRENT_TIMESTAMP, 1, x'00', 0)\"" \
    || fail "cannot mark the target's database (sqlite3 installed?)"
curl -skf -b "$work/$target_vm.jar" -H "Origin: $(base "$target_vm")" -H "X-CSRF-Token: $(cat "$work/$target_vm.csrf")" \
    -H 'Content-Type: application/json' -d "$(jq -n --arg p "$passphrase" '{passphrase: $p}')" \
    -o "$work/future.tar.zst.age" "$(base "$target_vm")/api/v1/configuration/backup/download" || fail "no backup of VM $target_vm"
on "$target_vm" "sudo -u zendiloos sqlite3 /var/lib/zendiloos/zendiloos.db 'DELETE FROM _sqlx_migrations WHERE version = 9999'"
[[ $(restore "$source_vm" /configuration/restore "$work/future.tar.zst.age" "$passphrase") == 422 ]] \
    && jq -e '.code == "backup_too_new"' "$work/restored.json" >/dev/null || fail "taken: $(cat "$work/restored.json")"
check "refused: $(jq -r .detail "$work/restored.json")"

say "Passed: download, schedule with retention, refusals, a restore onto a fresh machine with its accounts, SMB passwords and settings"
