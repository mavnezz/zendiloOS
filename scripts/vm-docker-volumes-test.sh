#!/usr/bin/env bash
# Tests Docker's volumes and their backups end to end in a test VM, through
# the HTTPS API:
#
#   1. a project with a named volume: the volume is listed with its project
#      and the running container that uses it, and its size is counted
#   2. a backup plan into a folder of a share, and "back up now": the
#      container is stopped meanwhile and runs again afterwards
#   3. the archive and its manifest lie in the share, readable by tar and
#      zstd, owners and links kept, the manifest's SHA-256 the archive's,
#      both under the share's inherited ACL, none root's alone
#   4. the data changes; restoring under its own name is refused while the
#      container runs unless it may be stopped, then brings the data back
#      and the container runs again
#   5. restoring into another volume
#   6. an archive changed in the share is refused: its SHA-256 is not the
#      one recorded, and the volume stays as it was
#   7. retention: with two kept, the third backup removes the oldest
#      archive and its manifest, and a look-alike file nobody recorded stays
#   8. everything made is removed again
#
# Needs the VM running with the package installed and set up, Docker's data
# on a pool and the image alpine:3.20 (fetched if missing); account and
# password as for vm-storage-test.sh. Runs in the second VM by default (the
# first holds a LAN test). Puts everything back.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
share=volbackups
project=voltest
volume=${project}_data
copy=${project}_copy
container=$project-db-1
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

sign_in() {
    curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf"
}

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
    local id=$1
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 900
    call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")'
}

job_succeeded() {
    local state
    state=$(job_end "$1")
    [[ $state == succeeded* ]] || fail "job $1: $state"
}

running() { [[ $(vm "sudo docker inspect -f '{{.State.Running}}' $container") == true ]]; }
started_at() { vm "sudo docker inspect -f '{{.State.StartedAt}}' $container"; }
inside() { vm "sudo docker exec $container sh -c '$1'"; }
# What the volume $1 holds at $2, seen from a container of its own.
in_volume() { vm "sudo docker run --rm -v $1:/v:ro alpine:3.20 sh -c '$2'"; }
volume_json() { call GET /docker/volumes | jq -c --arg v "$1" '.[] | select(.name == $v)'; }

back_up_now() {
    [[ $(status POST "/docker/volumes/$volume/backups") == 202 ]] || fail "back up: $(cat "$work/last")"
    job_succeeded "$(jq -r .id "$work/last")"
}

restore() {
    status POST "/docker/backups/$1/restore" "$2"
}

remove_project() {
    if call GET "/docker/projects/$project" | jq -e .name >/dev/null 2>&1; then
        job_succeeded "$(call DELETE "/docker/projects/$project" | jq -r .id)"
    fi
}

cleanup() {
    set +e
    call DELETE "/docker/volumes/$volume/backup-plan" >/dev/null
    remove_project
    vm "sudo docker volume rm -f $volume $copy" >/dev/null 2>&1
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" >/dev/null
    done
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
sign_in || fail "cannot sign in"
me=$(call GET /auth/session | jq -er .account.id)
pool=$(call GET /apps/docker | jq -er .volume.pool) || fail "Docker's data is not on a pool"
vm 'sudo docker image inspect alpine:3.20 >/dev/null 2>&1 || sudo docker pull -q alpine:3.20' >/dev/null

say "Cleaning up an earlier run"
call DELETE "/docker/volumes/$volume/backup-plan" >/dev/null || true
remove_project
vm "sudo docker volume rm -f $volume $copy" >/dev/null 2>&1 || true
for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
done

say "1. A project with a named volume: listed with its container, counted"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
[[ $(status POST "/files/shares/$share_id/folders" '{"path":"compose"}') == 201 ]] || fail "folder compose"
[[ $(status POST "/files/shares/$share_id/folders" '{"path":"docker-backups"}') == 201 ]] || fail "folder backups"
number=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .volume')
backups="/volume$number/$share/docker-backups/$volume"
compose=$'services:\n  db:\n    image: alpine:3.20\n    restart: unless-stopped\n    command: ["sleep", "infinity"]\n    volumes:\n      - data:/data\nvolumes:\n  data: {}\n'
request=$(jq -n --arg name "$project" --argjson share "$share_id" --arg compose "$compose" \
    '{name:$name,share_id:$share,folder:"compose",compose:$compose,start:true,accept_risks:true}')
[[ $(status POST /docker/projects "$request") == 201 ]] || fail "create: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
wait_for "the container" running 60
inside 'echo first > /data/state && mkdir -p /data/sub && ln -sf ../state /data/sub/link && chown 1234:4321 /data/state && dd if=/dev/urandom of=/data/blob bs=1M count=4 2>/dev/null'
listed=$(volume_json "$volume")
[[ $(jq -r .project <<<"$listed") == "$project" ]] || fail "the volume's project: $listed"
[[ $(jq -c .used_by <<<"$listed") == "[{\"name\":\"$container\",\"running\":true}]" ]] || fail "used by: $listed"
[[ $(jq -r .backed_up_here <<<"$listed") == true ]] || fail "not backed up here: $listed"
size=$(call GET /docker/volumes/sizes | jq -r --arg v "$volume" '.[] | select(.name == $v) | .bytes')
(( size >= 4 * 1024 * 1024 )) || fail "the volume's size: $size"

say "2. A plan, and a backup at once: the container stops meanwhile"
hour=$(( ($(vm 'date +%-H') + 12) % 24 ))
plan=$(jq -n --argjson share "$share_id" --argjson hour "$hour" \
    '{share_id:$share,folder:"docker-backups",frequency:"daily",hour:$hour,keep:2}')
[[ $(status PUT "/docker/volumes/$volume/backup-plan" "$plan") == 200 ]] || fail "plan: $(cat "$work/last")"
[[ $(jq -r .stop_containers "$work/last") == true ]] || fail "the plan does not stop the containers"
before=$(started_at)
back_up_now
wait_for "the container again" running 60
[[ $(started_at) != "$before" ]] || fail "the container was not stopped and started again"
backup=$(call GET "/docker/volumes/$volume/backups" | jq -c '.[0]')
[[ $(jq -r .state <<<"$backup") == succeeded ]] || fail "the backup: $backup"
first=$(jq -r .id <<<"$backup")
[[ $(volume_json "$volume" | jq -r .last_backup.id) == "$first" ]] || fail "the list does not show the backup"

say "3. The archive and its manifest in the share"
archive="/volume$number/$share/$(jq -r .archive <<<"$backup")"
manifest="/volume$number/$share/$(jq -r .manifest <<<"$backup")"
[[ $archive == "$backups/$volume-"*.tar.zst ]] || fail "the archive's name: $archive"
listing=$(vm "sudo zstd -dc $archive | tar -tvf -")
grep -q ' 1234/4321 .* state$' <<<"$listing" || fail "the owner of state: $listing"
grep -q 'sub/link -> ../state' <<<"$listing" || fail "the link: $listing"
sum=$(vm "sudo sha256sum $archive" | cut -d' ' -f1)
[[ $sum == $(jq -r .sha256 <<<"$backup") ]] || fail "the recorded SHA-256"
[[ $(vm "sudo cat $manifest" | jq -r '.volume + " " + .sha256 + " " + .labels["com.docker.compose.project"]') \
    == "$volume $sum $project" ]] || fail "the manifest: $(vm "sudo cat $manifest")"
for file in "$archive" "$manifest"; do
    vm "sudo getfacl -p $file" | grep -q "^user:$user:rw" || fail "$file lacks the share's ACL"
    [[ $(vm "sudo stat -c %a $file") != 600 ]] || fail "$file is root's alone"
done
[[ $(status GET "/files/shares/$share_id/download?path=$(jq -r .manifest <<<"$backup")") == 200 ]] \
    || fail "the administrator cannot read the manifest in the share"

say "4. The data changes; restored under its own name"
inside 'echo second > /data/state && touch /data/later'
in_use=$(restore "$first" '{"target":"own_name"}')
[[ $in_use == 409 ]] && jq -e '.code == "volume_in_use"' "$work/last" >/dev/null \
    || fail "restored while its container runs: $in_use $(cat "$work/last")"
[[ $(restore "$first" '{"target":"own_name","stop_containers":true}') == 202 ]] \
    || fail "restore: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
wait_for "the container after the restore" running 60
[[ $(inside 'cat /data/state') == first ]] || fail "the data was not restored"
inside 'test ! -e /data/later' || fail "what came later stayed"
[[ $(inside 'stat -c %u:%g /data/state') == 1234:4321 ]] || fail "the owner was not restored"
[[ $(inside 'readlink /data/sub/link') == ../state ]] || fail "the link was not restored"
vm "sudo ls /volume$number/@docker/volumes/$volume" | grep -q zendiloos-restore && fail "a fresh folder stayed"

say "5. Restored into another volume"
vm "sudo docker volume create $copy" >/dev/null
[[ $(restore "$first" "{\"target\":\"existing\",\"volume\":\"$copy\"}") == 202 ]] \
    || fail "restore into $copy: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
[[ $(in_volume "$copy" 'cat /v/state') == first ]] || fail "$copy does not hold the backup"

say "6. An archive changed in the share is refused"
inside 'echo third > /data/state'
vm "echo tampered | sudo tee -a $archive >/dev/null"
[[ $(restore "$first" '{"target":"own_name","stop_containers":true}') == 202 ]] \
    || fail "restore: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == failed*SHA-256* ]] || fail "a changed archive was restored: $state"
wait_for "the container after the refusal" running 60
[[ $(inside 'cat /data/state') == third ]] || fail "the volume changed"

say "7. Retention keeps two, and only removes what it recorded"
vm "echo not ours | sudo tee $backups/$volume-19990101-000000.tar.zst >/dev/null"
sleep 2
back_up_now
sleep 2
back_up_now
kept=$(call GET "/docker/volumes/$volume/backups" | jq '[.[] | select(.state == "succeeded")] | length')
[[ $kept == 2 ]] || fail "$kept backups are kept"
vm "sudo test ! -e $archive && sudo test ! -e $manifest" || fail "the oldest backup stayed"
[[ $(vm "sudo ls $backups" | grep -c '\.tar\.zst$') == 3 ]] || fail "in the folder: $(vm "sudo ls $backups")"
vm "sudo test -f $backups/$volume-19990101-000000.tar.zst" || fail "a file nobody recorded was removed"

say "8. Cleaning up"
[[ $(status DELETE "/docker/volumes/$volume/backup-plan") == 204 ]] || fail "the plan: $(cat "$work/last")"
remove_project
[[ $(status DELETE "/docker/volumes/$volume") == 204 ]] || fail "removing $volume: $(cat "$work/last")"
[[ $(status DELETE "/docker/volumes/$copy") == 204 ]] || fail "removing $copy: $(cat "$work/last")"
[[ -z $(volume_json "$volume") ]] || fail "$volume is still listed"

say "All Docker volume tests passed"
