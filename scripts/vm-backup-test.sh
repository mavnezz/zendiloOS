#!/usr/bin/env bash
# Backup tasks end to end in the second test VM, through the HTTPS API,
# with rsync, sshd and the files themselves looked at inside the VM:
#
#   1. the plugin installs (rsync, the SSH client and rclone, as a job); a
#      second install is refused
#   2. a share on one pool (apps) is backed up to a share on the other
#      (tank) as a mirror: the same bytes, *.tmp left out, links out of the
#      source (to /etc, to a file of root's) neither followed nor copied,
#      nothing of /etc or the secret anywhere in the target; a file deleted
#      in the source goes from the mirror into removed/<stamp>/
#   3. the same share as versions, three runs with a change in between:
#      each version complete under its own name, none left .incomplete,
#      unchanged files hard links to the version before, the changed one
#      not; retention (keep 2) removes the oldest after the third run
#   4. a USB-like drive (ext4, scripts/vm.sh removable-attach) is a target:
#      a task "when plugged in" runs once the drive is pulled out and
#      plugged in again, and its version lies on the drive
#   5. a test-only rsync daemon on 127.0.0.1:8873 and a test user of the
#      VM's sshd: a push to the daemon with a password and a bandwidth
#      limit, during which the password never shows in ps or in the unit's
#      properties; a push over SSH with the NAS's key and the host key
#      fetched, compared with the server's own and pinned; a pinned key
#      that is not the server's refuses the run (host_key_changed)
#   6. a pull from the daemon into a share: the same bytes, the server's
#      absolute link left out, nothing written outside the target
#   7. the first version restored into another folder: the same bytes as
#      then; restoring again keeps (skip) or replaces what is there
#   8. an encrypted backup with rclone (its crypt over a share's folder):
#      the password never in ps nor sent back, neither a name nor a file's
#      contents in plain text in the target (versions' names included);
#      two versions listed and browsed decrypted; the older one restored
#      with the same bytes as the source then, into the share's group; the
#      password cannot be changed afterwards
#
# Google Drive and OneDrive themselves are not reached here: their
# sign-in and rclone's backends are tested against fakes only.
#
# Needs the VM running (with the monitor, for the drive: restart it once if
# removable-attach says so), the package installed and set up with the
# pools apps and tank, the network for apt in the VM, and jq, sha256sum and
# mkfs.ext4 on this machine; account and password as for
# vm-storage-test.sh. The plugin and the NAS's SSH key stay; the test's
# tasks, shares, drive, daemon, user and files go, also when it fails.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
src_share="ztestbksrc"
dst_share="ztestbkdst"
src_pool=${ZENDILOOS_BACKUP_SOURCE_POOL:-apps}
dst_pool=${ZENDILOOS_BACKUP_TARGET_POOL:-tank}
server_user="zbackup"
daemon_port=8873
daemon_password="ztest-$(head -c 9 /dev/urandom | od -An -tx1 | tr -d ' \n')"
drive_id="zbk"
work=$(mktemp -d)
drive_attached=false

say() { printf '\033[1m%s\033[0m\n' "$*"; }
note() { printf '   %s\n' "$*"; }
ok() { printf '   \033[32mok\033[0m %s\n' "$*"; }
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

# Makes a task from the JSON $1; its id.
make_task() {
    [[ $(status POST /backup/tasks "$1") == 201 ]] || fail "the task $(jq -r .name <<<"$1"): $(cat "$work/last")"
    jq -er .id "$work/last"
}

# Starts task $1 and waits for its job; prints how it ended.
run_task() {
    [[ $(status POST "/backup/tasks/$1/run") == 202 ]] || fail "back up task $1: $(cat "$work/last")"
    job_end "$(jq -r .id "$work/last")" "${2:-900}"
}

run_succeeds() {
    local state
    state=$(run_task "$@")
    [[ $state == succeeded* ]] || fail "task $1: $state"
}

# The checksums of every file below $1 in the VM (paths relative, sorted),
# without the marker and what matches $2 (a grep pattern) if given.
sums() {
    vm "sudo sh -c 'cd \"$1\" && find . -type f ! -name .zendiloos-backup -print0 | sort -z | xargs -0r sha256sum'" \
        | { if [[ -n ${2:-} ]]; then grep -v -- "$2"; else cat; fi; }
}

# The names in the folder $1 of the VM, one per line.
names() { vm "sudo ls -1A \"$1\" 2>/dev/null" || true; }

share_path() { call GET /shares | jq -r --arg s "$1" '.[] | select(.name == $s) | .path'; }
share_id() { call GET /shares | jq -r --arg s "$1" '.[] | select(.name == $s) | .id'; }

delete_tasks() {
    local id
    for id in $(call GET /backup/tasks 2>/dev/null | jq -r '.[]? | select(.name | startswith("ztest")) | .id'); do
        call POST "/backup/tasks/$id/stop" >/dev/null 2>&1 || true
        call DELETE "/backup/tasks/$id" >/dev/null || true
    done
}

delete_share() {
    local id
    for id in $(call GET /shares | jq -r --arg s "$1" '.[] | select(.name == $s) | .id'); do
        job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$1\"}" | jq -r .id)"
    done
}

clean_vm() {
    vm "sudo sh -c '
        [ -f /run/ztest-rsyncd.pid ] && kill \$(cat /run/ztest-rsyncd.pid) 2>/dev/null
        rm -f /run/ztest-rsyncd.pid /etc/ztest-rsyncd.conf /etc/ztest-rsyncd.secrets /root/ztest-backup-secret
        rm -rf /srv/ztest-rsync
        id $server_user >/dev/null 2>&1 && userdel -r $server_user 2>/dev/null
        true'" || true
}

clean() {
    delete_tasks
    delete_share "$src_share"
    delete_share "$dst_share"
    clean_vm
}

cleanup() {
    set +e
    if $drive_attached; then "$root/scripts/vm.sh" removable-detach "$drive_id" >/dev/null 2>&1; fi
    [[ -s $work/admin.csrf ]] && clean
    rm -rf "$work"
}
trap cleanup EXIT

for tool in jq sha256sum curl; do command -v "$tool" >/dev/null || fail "$tool is needed on this machine"; done
mkfs_ext4=$(command -v mkfs.ext4 || echo /usr/sbin/mkfs.ext4)
[[ -x $mkfs_ext4 ]] || fail "mkfs.ext4 is needed on this machine"

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
me=$(call GET /auth/session | jq -er .account.id) || fail "no account"
pools=$(call GET /storage/pools | jq -r '.[].name')
for pool in "$src_pool" "$dst_pool"; do
    grep -qx "$pool" <<<"$pools" || fail "no pool $pool (pools: $(tr '\n' ' ' <<<"$pools"))"
done

say "Cleaning up an earlier run"
clean

say "1. The plugin installs rsync, the SSH client and rclone as a job"
if [[ $(call GET /backup/plugin | jq -r '.installed and .rclone') != true ]]; then
    [[ $(status POST /backup/plugin/install) == 202 ]] || fail "install: $(cat "$work/last")"
    job_succeeded "$(jq -r .id "$work/last")" 1800
    ok "installed"
else
    note "installed already"
fi
[[ $(status POST /backup/plugin/install) == 409 ]] && jq -e '.code == "backup_installed"' "$work/last" >/dev/null \
    || fail "a second install was not refused: $(cat "$work/last")"
ok "a second install is refused"
for package in rsync openssh-client rclone; do
    vm "dpkg-query -W -f='\${Status}' $package 2>/dev/null | grep -q 'install ok installed'" || fail "$package is not installed"
done
ok "rsync, openssh-client and rclone are installed"

say "2. A share on $src_pool backed up to a share on $dst_pool as a mirror"
for pair in "$src_share:$src_pool" "$dst_share:$dst_pool"; do
    call POST /shares "$(jq -n --arg n "${pair%%:*}" --arg pool "${pair##*:}" --argjson me "$me" \
        '{name: $n, pool: $pool, access: [{kind: "account", id: $me, access: "read_write"}]}')" | jq -er .id >/dev/null \
        || fail "the share ${pair%%:*}"
done
src=$(share_path "$src_share")
dst=$(share_path "$dst_share")
src_id=$(share_id "$src_share")
dst_id=$(share_id "$dst_share")
note "$src → $dst"
secret="ZTEST-SECRET-$RANDOM$RANDOM"
vm "sudo sh -c '
    set -e
    mkdir -p \"$src/albums/2024 summer\" \"$src/docs\"
    head -c 6M /dev/urandom > \"$src/albums/2024 summer/beach.raw\"
    head -c 300K /dev/urandom > \"$src/albums/2024 summer/dunes.raw\"
    echo v1 > \"$src/docs/notes.txt\"
    echo old > \"$src/docs/old.txt\"
    echo temporary > \"$src/docs/scratch.tmp\"
    ln -s ../docs/notes.txt \"$src/albums/notes-link\"
    ln -s /etc \"$src/escape\"
    echo $secret > /root/ztest-backup-secret
    chmod 600 /root/ztest-backup-secret
    ln -s /root/ztest-backup-secret \"$src/secret-link\"'"
mirror=$(make_task "$(jq -n --argjson s "$src_id" --argjson d "$dst_id" '{
    name: "ztest mirror", source: {kind: "share", share_id: $s, path: ""},
    target: {kind: "share", share_id: $d, path: ""},
    mode: {kind: "mirror", keep_changed_days: 7}, excludes: ["*.tmp"], hidden: true}')")
[[ $(call GET "/backup/tasks/$mirror" | jq -r .folder) == "ztest mirror" ]] || fail "the task's folder"
run_succeeds "$mirror"
current="$dst/ztest mirror/current"
[[ $(sums "$src" '\.tmp$') == "$(sums "$current")" ]] || fail "the mirror differs from the source"
ok "the mirror holds the same bytes, *.tmp left out"
[[ -n $(names "$dst/ztest mirror" | grep -x .zendiloos-backup) ]] || fail "no marker"
ok "the task's folder is marked as a backup"
vm "sudo test -L \"$current/albums/notes-link\"" || fail "a link within the source was not kept as a link"
ok "a link within the source is kept as a link"
for planted in escape secret-link; do
    vm "sudo test ! -e \"$current/$planted\" -a ! -L \"$current/$planted\"" || fail "$planted was copied"
done
vm "sudo grep -rqs '$secret' \"$dst\"" && fail "the secret behind a link reached the target"
vm "sudo test ! -e \"$current/escape/passwd\"" || fail "/etc was read through a link"
ok "links out of the source (to /etc, to root's file) neither followed nor copied"
vm "sudo rm \"$src/docs/old.txt\""
sleep 1
run_succeeds "$mirror"
vm "sudo test ! -e \"$current/docs/old.txt\"" || fail "a file deleted in the source stayed in the mirror"
kept=$(names "$dst/ztest mirror/removed")
[[ $(wc -l <<<"$kept") == 1 ]] || fail "removed/ holds: $kept"
vm "sudo grep -qx old \"$dst/ztest mirror/removed/$kept/docs/old.txt\"" || fail "the deleted file was not kept"
ok "a deleted file went from the mirror into removed/$kept/"
[[ $(call GET "/backup/tasks/$mirror/runs" | jq -r '.[0].outcome') == succeeded ]] || fail "the run was not logged"
run_id=$(call GET "/backup/tasks/$mirror/runs" | jq -r '.[0].id')
call GET "/backup/runs/$run_id/transfers" | jq -e '.transfers | map(.change) | any(startswith("*deleting"))' >/dev/null \
    || fail "the transfer list does not name the deletion"
ok "the run is logged with its transfer list"

say "3. The same share as versions, three runs with a change between"
versioned=$(make_task "$(jq -n --argjson s "$src_id" --argjson d "$dst_id" '{
    name: "ztest versions", source: {kind: "share", share_id: $s, path: ""},
    target: {kind: "share", share_id: $d, path: ""},
    mode: {kind: "versioned", keep_last: 2, keep_days: null}, excludes: ["*.tmp"], hidden: true}')")
folder="$dst/ztest versions"
run_succeeds "$versioned"
first=$(names "$folder" | grep -E '^[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{6}Z$')
[[ $(wc -l <<<"$first") == 1 ]] || fail "after one run: $(names "$folder" | tr '\n' ' ')"
first_sums=$(sums "$folder/$first")
[[ $first_sums == "$(sums "$src" '\.tmp$')" ]] || fail "the first version differs from the source"
ok "version $first is complete"
vm "echo 'v2, longer now' | sudo tee \"$src/docs/notes.txt\" >/dev/null"
sleep 2
run_succeeds "$versioned"
second=$(names "$folder" | grep -E 'Z$' | grep -vx "$first")
[[ $(wc -l <<<"$second") == 1 ]] || fail "after two runs: $(names "$folder" | tr '\n' ' ')"
names "$folder" | grep -q '\.incomplete$' && fail "a version was left .incomplete"
ok "version $second is complete, none left .incomplete"
inode() { vm "sudo stat -c %i \"$1\""; }
[[ $(inode "$folder/$first/albums/2024 summer/beach.raw") == "$(inode "$folder/$second/albums/2024 summer/beach.raw")" ]] \
    || fail "an unchanged file is not a hard link to the version before"
[[ $(inode "$folder/$first/docs/notes.txt") != "$(inode "$folder/$second/docs/notes.txt")" ]] \
    || fail "the changed file is linked to the version before"
vm "sudo grep -qx v1 \"$folder/$first/docs/notes.txt\"" || fail "the first version changed"
ok "unchanged files are hard links to the version before, the changed one is not"
sleep 2
run_succeeds "$versioned"
left=$(names "$folder" | grep -E 'Z$')
[[ $(wc -l <<<"$left") == 2 ]] || fail "retention kept: $(tr '\n' ' ' <<<"$left")"
grep -qx "$first" <<<"$left" && fail "retention kept the oldest version"
ok "retention keeps the newest two: $(tr '\n' ' ' <<<"$left")"
versions=$(call GET "/backup/tasks/$versioned/versions")
[[ $(jq '.versions | length' <<<"$versions") == 2 ]] || fail "the API lists: $versions"
ok "the API lists the two versions"

say "4. A USB-like drive as target, backed up when plugged in"
truncate -s 256M "$work/zbk.img"
"$mkfs_ext4" -q -L ZBACKUP "$work/zbk.img"
"$root/scripts/vm.sh" removable-attach "$work/zbk.img" "$drive_id" || fail "cannot plug the drive in"
drive_attached=true
drive_of() { call GET /storage/removable | jq -r '.[] | select(.label == "ZBACKUP" and .state == "mounted") | .id'; }
wait_for "the drive mounted" '[[ -n $(drive_of) ]]' 120
uuid=$(drive_of)
note "drive $uuid"
on_drive=$(make_task "$(jq -n --argjson s "$src_id" --arg drive "$uuid" '{
    name: "ztest drive", source: {kind: "share", share_id: $s, path: "albums"},
    target: {kind: "drive", drive: $drive, path: ""},
    mode: {kind: "versioned", keep_last: 5, keep_days: null}, on_plug: true, excludes: [], hidden: true}')")
sleep 70
[[ $(call GET "/backup/tasks/$on_drive/runs" | jq length) == 0 ]] || fail "it ran although the drive was plugged in when it was made"
ok "made while plugged in: no run"
"$root/scripts/vm.sh" removable-detach "$drive_id"
drive_attached=false
wait_for "the drive gone" '[[ -z $(call GET /storage/removable | jq -r ".[] | select(.label == \"ZBACKUP\") | .id") ]]' 120
"$root/scripts/vm.sh" removable-attach "$work/zbk.img" "$drive_id" || fail "cannot plug the drive in again"
drive_attached=true
plugged_run() { call GET "/backup/tasks/$on_drive/runs" | jq -r '.[0] | select(.trigger == "plug") | .outcome'; }
wait_for "a run when plugged in" '[[ $(plugged_run) =~ ^(succeeded|failed|partial)$ ]]' 300
[[ $(plugged_run) == succeeded ]] || fail "the run when plugged in: $(call GET "/backup/tasks/$on_drive/runs" | jq -c '.[0]')"
on_stick=$(names "/run/zendiloos-removable/$uuid/ztest drive" | grep -E 'Z$')
[[ $(sums "/run/zendiloos-removable/$uuid/ztest drive/$on_stick") == "$(sums "$src/albums")" ]] \
    || fail "the drive's version differs from the source"
ok "plugged in again, it backed up: $on_stick on the drive"

say "5. A test-only rsync daemon and SSH user on the VM itself"
cat >"$work/rsyncd.conf" <<EOF
pid file = /run/ztest-rsyncd.pid
port = $daemon_port
address = 127.0.0.1
use chroot = yes
uid = root
gid = root
[ztest]
    path = /srv/ztest-rsync
    read only = no
    auth users = $server_user
    secrets file = /etc/ztest-rsyncd.secrets
EOF
vm "sudo tee /etc/ztest-rsyncd.conf >/dev/null" <"$work/rsyncd.conf"
printf '%s:%s\n' "$server_user" "$daemon_password" | vm "sudo sh -c 'umask 077; cat > /etc/ztest-rsyncd.secrets'"
vm "sudo sh -c '
    set -e
    mkdir -p /srv/ztest-rsync/pushed /srv/ztest-rsync/serverdata/www
    head -c 2M /dev/urandom > /srv/ztest-rsync/serverdata/www/site.tar
    echo hello > /srv/ztest-rsync/serverdata/www/index.html
    ln -s /etc /srv/ztest-rsync/serverdata/etc
    rsync --daemon --config=/etc/ztest-rsyncd.conf'"
wait_for "the daemon" "vm 'ss -ltn | grep -q 127.0.0.1:$daemon_port'" 20
ok "rsync daemon on 127.0.0.1:$daemon_port"
vm "sudo useradd -m -s /bin/sh $server_user && sudo -u $server_user mkdir -p /home/$server_user/backups /home/$server_user/.ssh"
public_key=$(call POST /backup/key | jq -er .public_key) || fail "no public key"
printf '%s\n' "$public_key" | vm "sudo -u $server_user sh -c 'umask 077; cat > ~/.ssh/authorized_keys'"
ok "user $server_user with the NAS's key: ${public_key:0:40}…"

daemon_server=$(jq -n --arg u "$server_user" --argjson p "$daemon_port" \
    '{transport: "daemon", host: "127.0.0.1", port: $p, user: $u, module: "ztest"}')
check=$(jq -n --argjson s "$daemon_server" --arg pw "$daemon_password" '{server: $s, path: "serverdata", password: $pw}')
[[ $(status POST /backup/servers/check "$check") == 200 ]] || fail "the daemon cannot be reached: $(cat "$work/last")"
ok "the daemon answers with the password"
push=$(make_task "$(jq -n --argjson s "$src_id" --argjson server "$daemon_server" --arg pw "$daemon_password" '{
    name: "ztest push daemon", source: {kind: "share", share_id: $s, path: ""},
    target: {kind: "server", path: "pushed"}, server: $server, password: $pw,
    mode: {kind: "versioned", keep_last: 3, keep_days: null}, excludes: ["*.tmp"], hidden: true, bwlimit_kib: 1024}')")
call GET "/backup/tasks/$push" | grep -q "$daemon_password" && fail "the API sent the password back"
ok "the password is stored, never sent back"
[[ $(status POST "/backup/tasks/$push/run") == 202 ]] || fail "push: $(cat "$work/last")"
push_job=$(jq -r .id "$work/last")
seen=0
for _ in $(seq 1 60); do
    [[ $(call GET "/jobs/$push_job" | jq -r .state) == running ]] || break
    vm "ps -eo args" >>"$work/ps"
    vm "sudo systemctl show 'zendiloos-backup-run-$push.service' 2>/dev/null" >>"$work/ps"
    seen=$((seen + 1))
    sleep 0.3
done
state=$(job_end "$push_job")
[[ $state == succeeded* ]] || fail "push to the daemon: $state"
(( seen > 2 )) || note "the run ended quickly: ps was looked at $seen times"
grep -q -- "$daemon_password" "$work/ps" && fail "the password showed in ps or in the unit's properties"
grep -q 'backup-unit' "$work/ps" || note "the unit itself was not seen in ps"
ok "the password never showed in ps or the unit's properties ($seen looks)"
pushed=$(names "/srv/ztest-rsync/pushed/ztest push daemon" | grep -E 'Z$')
[[ $(wc -l <<<"$pushed") == 1 ]] || fail "on the daemon: $(names "/srv/ztest-rsync/pushed/ztest push daemon" | tr '\n' ' ')"
names "/srv/ztest-rsync/pushed/ztest push daemon" | grep -q '\.incomplete$' && fail "an .incomplete version was left on the daemon"
[[ $(sums "/srv/ztest-rsync/pushed/ztest push daemon/$pushed") == "$(sums "$src" '\.tmp$')" ]] \
    || fail "the daemon's version differs from the source"
ok "pushed to the daemon as version $pushed, renamed once complete"

keys=$(call POST /backup/servers/host-keys '{"host":"127.0.0.1","port":22}')
line=$(jq -er '.[] | select(.line | startswith("ssh-ed25519 ")) | .line' <<<"$keys") || fail "no ed25519 key offered: $keys"
fingerprint=$(jq -er '.[] | select(.line | startswith("ssh-ed25519 ")) | .fingerprint' <<<"$keys")
own=$(vm "ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub" | awk '{print $2}')
[[ $fingerprint == "$own" ]] || fail "the fingerprint $fingerprint is not the server's own $own"
ok "the host key's fingerprint is the server's own: $fingerprint"
ssh_server=$(jq -n --arg u "$server_user" --arg k "$line" --arg f "$fingerprint" \
    '{transport: "ssh", host: "127.0.0.1", port: 22, user: $u, host_key: $k, fingerprint: $f}')
over_ssh=$(make_task "$(jq -n --argjson s "$src_id" --argjson server "$ssh_server" '{
    name: "ztest push ssh", source: {kind: "share", share_id: $s, path: "albums"},
    target: {kind: "server", path: "backups"}, server: $server,
    mode: {kind: "mirror", keep_changed_days: null}, excludes: [], hidden: true}')")
run_succeeds "$over_ssh"
[[ $(sums "/home/$server_user/backups/ztest push ssh/current") == "$(sums "$src/albums")" ]] \
    || fail "the copy over SSH differs from the source"
ok "pushed over SSH with the NAS's key and the pinned host key"
# Another ed25519 key than the server's: ssh is offered the server's own.
wrong=$(call GET "/backup/tasks/$over_ssh" | jq '{
    name, source, target,
    server: (.server + {host_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
                        fingerprint: "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"}),
    mode, schedule, on_plug, excludes, hidden, bwlimit_kib, notify_success}')
[[ $(status PUT "/backup/tasks/$over_ssh" "$wrong") == 200 ]] || fail "pinning another key: $(cat "$work/last")"
state=$(run_task "$over_ssh")
[[ $state == "failed host_key_changed"* ]] || fail "a key not the server's did not refuse the run: $state"
ok "a pinned key that is not the server's refuses the run (host_key_changed)"

say "6. A pull from the daemon into a share"
pull=$(make_task "$(jq -n --argjson d "$dst_id" --argjson server "$daemon_server" --arg pw "$daemon_password" '{
    name: "ztest pull", source: {kind: "server", path: "serverdata"},
    target: {kind: "share", share_id: $d, path: "servers"}, server: $server, password: $pw,
    mode: {kind: "mirror", keep_changed_days: null}, excludes: [], hidden: true}')")
vm "sudo mkdir -p \"$dst/servers\""
run_succeeds "$pull"
[[ $(sums "$dst/servers/ztest pull/current") == "$(sums /srv/ztest-rsync/serverdata)" ]] \
    || fail "the pulled copy differs from the server's"
vm "sudo test ! -e \"$dst/servers/ztest pull/current/etc\" -a ! -L \"$dst/servers/ztest pull/current/etc\"" \
    || fail "the server's absolute link was taken"
vm "sudo test ! -e /etc/index.html -a ! -e /index.html" || fail "something was written outside the target"
ok "pulled the same bytes; the server's absolute link left out, nothing written outside"

say "7. The oldest version restored into another folder"
versions=$(call GET "/backup/tasks/$versioned/versions")
oldest=$(jq -r '.versions | last' <<<"$versions")
older_name=$(names "$folder" | grep -E 'Z$' | head -1)
expected=$(sums "$folder/$older_name")
restore=$(jq -n --argjson v "$oldest" --argjson d "$dst_id" '{version: $v, paths: [], existing: "skip",
    to: {kind: "share", share_id: $d, path: "restored"}}')
[[ $(status POST "/backup/tasks/$versioned/restore" "$restore") == 202 ]] || fail "restore: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
[[ $(sums "$dst/restored") == "$expected" ]] || fail "the restored files differ from the version"
ok "restored version $older_name into $dst_share/restored: the same bytes"
group=$(vm "stat -c %G \"$dst/restored/docs/notes.txt\"")
[[ $group == zendiloos-files ]] || fail "a restored file's group is $group"
vm "getfacl -p \"$dst/restored/docs/notes.txt\" 2>/dev/null | grep -q '^group:zendiloos-files:'" \
    || note "getfacl shows no named group entry (the share's inherited ACL)"
ok "restored files take the share's group"
vm "echo 'changed after the restore' | sudo tee \"$dst/restored/docs/notes.txt\" >/dev/null"
job_succeeded "$(call POST "/backup/tasks/$versioned/restore" "$restore" | jq -r .id)"
vm "sudo grep -qx 'changed after the restore' \"$dst/restored/docs/notes.txt\"" || fail "skip replaced a file"
replace=$(jq '.existing = "replace" | .paths = ["docs/notes.txt"]' <<<"$restore")
job_succeeded "$(call POST "/backup/tasks/$versioned/restore" "$replace" | jq -r .id)"
[[ $(sums "$dst/restored/docs") == "$(sums "$folder/$older_name/docs")" ]] || fail "replace did not replace"
ok "a restore again keeps (skip) or replaces what is there, as asked"

say "8. An encrypted backup with rclone, restored and decrypted"
crypt_password="ztest-crypt-$(head -c 9 /dev/urandom | od -An -tx1 | tr -d ' \n')"
crypt_salt="ztest-salt-$(head -c 6 /dev/urandom | od -An -tx1 | tr -d ' \n')"
plain="ZTEST-PLAIN-$RANDOM$RANDOM"
vm "echo $plain | sudo tee \"$src/docs/plain.txt\" >/dev/null"
before=$(sums "$src" '\.tmp$')
encrypted=$(make_task "$(jq -n --argjson s "$src_id" --argjson d "$dst_id" --arg pw "$crypt_password" --arg salt "$crypt_salt" '{
    name: "ztest encrypted", source: {kind: "share", share_id: $s, path: ""},
    target: {kind: "share", share_id: $d, path: ""},
    mode: {kind: "versioned", keep_last: 3, keep_days: null}, excludes: ["*.tmp"], hidden: true,
    encrypted: true, crypt_password: $pw, crypt_salt: $salt}')")
shown=$(call GET "/backup/tasks/$encrypted")
grep -qF -e "$crypt_password" -e "$crypt_salt" <<<"$shown" && fail "the API sent the crypt's password back"
[[ $(jq -r .encrypted <<<"$shown") == true ]] || fail "the task is not encrypted: $shown"
ok "the crypt's password and salt are stored, never sent back"
[[ $(status POST "/backup/tasks/$encrypted/run") == 202 ]] || fail "the encrypted run: $(cat "$work/last")"
crypt_job=$(jq -r .id "$work/last")
seen=0
for _ in $(seq 1 60); do
    [[ $(call GET "/jobs/$crypt_job" | jq -r .state) == running ]] || break
    vm "ps -eo args" >>"$work/ps-crypt"
    vm "sudo systemctl show 'zendiloos-backup-run-$encrypted.service' 2>/dev/null" >>"$work/ps-crypt"
    seen=$((seen + 1))
    sleep 0.3
done
state=$(job_end "$crypt_job")
[[ $state == succeeded* ]] || fail "the encrypted run: $state"
grep -qF -e "$crypt_password" -e "$crypt_salt" "$work/ps-crypt" && fail "the crypt's password showed in ps or the unit's properties"
grep -q 'rclone' "$work/ps-crypt" || note "rclone itself was not seen in ps ($seen looks)"
ok "the crypt's password never showed in ps or the unit's properties ($seen looks)"
enc="$dst/ztest encrypted"
stored=$(vm "sudo find \"$enc\" -mindepth 1 -printf '%P\n'")
[[ -n $stored ]] || fail "the encrypted target is empty"
for name in albums docs notes.txt beach.raw "2024 summer" plain.txt current .zendiloos-backup; do
    grep -qF -- "$name" <<<"$stored" && fail "the name $name shows in the encrypted target"
done
grep -qE '[0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{6}Z' <<<"$stored" && fail "a version's name shows in the encrypted target"
vm "sudo grep -rqsF '$plain' \"$enc\"" && fail "a file's contents show in the encrypted target"
vm "sudo grep -rqsF 'v2, longer now' \"$enc\"" && fail "a file's contents show in the encrypted target"
ok "the target holds no name and no contents in plain text, versions' names included"
vm "echo 'v3, encrypted' | sudo tee \"$src/docs/notes.txt\" >/dev/null"
sleep 2
run_succeeds "$encrypted"
versions=$(call GET "/backup/tasks/$encrypted/versions")
[[ $(jq '.versions | length' <<<"$versions") == 2 ]] || fail "through the crypt the API lists: $versions"
newest=$(jq -r '.versions[0]' <<<"$versions")
oldest=$(jq -r '.versions | last' <<<"$versions")
browsed=$(call GET "/backup/tasks/$encrypted/browse?version=$newest&path=docs")
jq -e 'map(.name) | index("notes.txt")' <<<"$browsed" >/dev/null || fail "browsing the newest version: $browsed"
ok "two versions, listed and browsed decrypted"
restore=$(jq -n --argjson v "$oldest" --argjson d "$dst_id" '{version: $v, paths: [], existing: "replace",
    to: {kind: "share", share_id: $d, path: "decrypted"}}')
[[ $(status POST "/backup/tasks/$encrypted/restore" "$restore") == 202 ]] || fail "restore: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
[[ $(sums "$dst/decrypted") == "$before" ]] || fail "the decrypted files differ from the source as it was"
ok "the older version restored and decrypted: the same bytes as the source then"
group=$(vm "stat -c %G \"$dst/decrypted/docs/notes.txt\"")
[[ $group == zendiloos-files ]] || fail "a decrypted file's group is $group"
ok "decrypted files take the share's group"
some=$(jq -n --argjson v "$newest" --argjson d "$dst_id" '{version: $v, paths: ["docs"], existing: "replace",
    to: {kind: "share", share_id: $d, path: "decrypted-docs"}}')
job_succeeded "$(call POST "/backup/tasks/$encrypted/restore" "$some" | jq -r .id)"
[[ $(sums "$dst/decrypted-docs") == "$(sums "$src" '\.tmp$' | grep ' ./docs/')" ]] \
    || fail "the restored folder differs from the source's"
vm "sudo test ! -e \"$dst/decrypted-docs/albums\"" || fail "more than the folder asked for was restored"
ok "one folder of the newest version restored alone"
again=$(jq --arg pw "another-password" '{name, source, target, server, mode, schedule, on_plug, excludes, hidden,
    bwlimit_kib, notify_success, encrypted, crypt_password: $pw}' <<<"$shown")
[[ $(status PUT "/backup/tasks/$encrypted" "$again") == 422 ]] || fail "the crypt's password changed: $(cat "$work/last")"
ok "the crypt's password cannot be changed afterwards"

say "All backup checks passed"
