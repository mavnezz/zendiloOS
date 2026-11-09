#!/usr/bin/env bash
# Tests Docker's volumes and their backups end to end in a test VM, through
# the HTTPS API:
#
#   1. a project with two named volumes: listed with its project and the
#      running container that uses them, and their sizes counted
#   2. a backup plan into a folder of a share, and "back up now": the
#      container is stopped meanwhile and runs again afterwards; the plan
#      tells who may read the share
#   3. the archive and its manifest lie in the share, readable by tar and
#      zstd (written by a unit in its sandbox: seccomp's @system-service,
#      ProtectProc, PrivateIPC), owners and links kept, the manifest's
#      SHA-256 the archive's, both readable as the share's inherited ACL
#      says but writable by root only (no write in the ACL mask)
#   4. the data changes; restoring under its own name is refused while the
#      container runs unless it may be stopped, then brings the data back
#      and the container runs again
#   5. restoring into another volume
#   6. an archive changed in the share is refused: its SHA-256 is not the
#      one recorded, and the volume stays as it was
#   7. an archive a share user may write (as a share's ACL could let them)
#      is refused, the volume as it was
#   8. one container of two volumes, both with plans at the same hour and
#      backed up at once: one at a time, the container stopped from the
#      first's start to the second's end, and started only once
#   9. a restore whose unit is stopped midway: the old data stays, the
#      container runs again, and what the unit left beside the data goes
#      with the next restore
#  10. leftovers are swept: a backup's hidden file in its folder before the
#      next backup there, a restore's fresh folder when the agent starts
#  11. retention: with two kept, the third backup removes the oldest
#      archive and its manifest, and a look-alike file nobody recorded stays
#  12. a volume deleted outside zendiloOS: its backups and plan stay on the
#      Backups page, the volume marked deleted, and it is restored from
#      there under its own name
#  13. the database forgets two backups (as after setting zendiloOS up
#      anew) and the folder is imported: the backup only root may write is
#      recorded and restored, the one made group-writable is refused with
#      why; importing again records nothing; retention leaves the imported
#      backup alone, and removing it by hand removes its files
#  14. everything made is removed again
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
extra=${project}_extra
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
# How often the container was started since $1 (Docker's time).
starts_since() {
    vm "sudo timeout 3 docker events --since '$1' --until \"\$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)\" \
        --filter container=$container --filter event=start --format '{{.Status}}'" | grep -c start || true
}
started_at() { vm "sudo docker inspect -f '{{.State.StartedAt}}' $container"; }
inside() { vm "sudo docker exec $container sh -c '$1'"; }
# What the volume $1 holds at $2, seen from a container of its own.
in_volume() { vm "sudo docker run --rm -v $1:/v:ro alpine:3.20 sh -c '$2'"; }
volume_json() { call GET /docker/volumes | jq -c --arg v "$1" '.[] | select(.name == $v)'; }
# The Backups page's group of volume $1.
backup_group() { call GET /docker/backups | jq -c --arg v "$1" '.[] | select(.volume == $v)'; }
# Forgets the record of backup $1, as a database set up anew would not know it.
forget_record() {
    vm 'command -v sqlite3 >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq sqlite3 >/dev/null'
    vm "sudo sqlite3 -cmd '.timeout 10000' /var/lib/zendiloos/zendiloos.db 'DELETE FROM docker_backups WHERE id = $1'"
}

back_up_now() {
    [[ $(status POST "/docker/volumes/${1:-$volume}/backups") == 202 ]] || fail "back up: $(cat "$work/last")"
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
    call DELETE "/docker/volumes/$extra/backup-plan" >/dev/null
    call DELETE "/docker/volumes/$copy/backup-plan" >/dev/null
    remove_project
    vm "sudo docker volume rm -f $volume $extra $copy" >/dev/null 2>&1
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
call DELETE "/docker/volumes/$extra/backup-plan" >/dev/null || true
call DELETE "/docker/volumes/$copy/backup-plan" >/dev/null || true
remove_project
vm "sudo docker volume rm -f $volume $extra $copy" >/dev/null 2>&1 || true
for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
done

say "1. A project with two named volumes: listed with its container, counted"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
[[ $(status POST "/files/shares/$share_id/folders" '{"path":"compose"}') == 201 ]] || fail "folder compose"
[[ $(status POST "/files/shares/$share_id/folders" '{"path":"docker-backups"}') == 201 ]] || fail "folder backups"
number=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .volume')
backups="/volume$number/$share/docker-backups/$volume"
compose=$'services:\n  db:\n    image: alpine:3.20\n    restart: unless-stopped\n    command: ["sleep", "infinity"]\n    volumes:\n      - data:/data\n      - extra:/extra\nvolumes:\n  data: {}\n  extra: {}\n'
request=$(jq -n --arg name "$project" --argjson share "$share_id" --arg compose "$compose" \
    '{name:$name,share_id:$share,folder:"compose",compose:$compose,start:true,accept_risks:true}')
[[ $(status POST /docker/projects "$request") == 201 ]] || fail "create: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
wait_for "the container" running 60
inside 'echo first > /data/state && mkdir -p /data/sub && ln -sf ../state /data/sub/link && chown 1234:4321 /data/state && dd if=/dev/urandom of=/data/blob bs=1M count=4 2>/dev/null'
inside 'dd if=/dev/urandom of=/extra/blob bs=1M count=256 2>/dev/null'
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
readers=$(call GET "/docker/backup-shares/$share_id/readers")
jq -e --arg u "$user" '.accounts | index($u)' <<<"$readers" >/dev/null || fail "who may read the share: $readers"
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
vm "sudo zstd -tq $archive" || fail "zstd does not read the archive"
sum=$(vm "sudo sha256sum $archive" | cut -d' ' -f1)
[[ $sum == $(jq -r .sha256 <<<"$backup") ]] || fail "the recorded SHA-256"
[[ $(vm "sudo cat $manifest" | jq -r '.volume + " " + .sha256 + " " + .labels["com.docker.compose.project"]') \
    == "$volume $sum $project" ]] || fail "the manifest: $(vm "sudo cat $manifest")"
for file in "$archive" "$manifest"; do
    acl=$(vm "sudo getfacl -p $file")
    grep -q '^group:zendiloos-files:r' <<<"$acl" || fail "$file lacks the share's ACL: $acl"
    grep -q '^mask::r-' <<<"$acl" || fail "the share's writers may write $file: $acl"
    [[ $(vm "sudo stat -c '%U %a' $file") =~ ^root\ [0-7][0145][0145]$ ]] \
        || fail "only root may write $file: $(vm "sudo stat -c '%U %a' $file")"
    vm "sudo -u nobody test -w $file" && fail "others may write $file"
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

say "7. An archive a share user may write is refused"
back_up_now
writable=$(call GET "/docker/volumes/$volume/backups" | jq -c '.[0]')
writable_path="/volume$number/$share/$(jq -r .archive <<<"$writable")"
inside 'echo fourth > /data/state'
vm "sudo setfacl -m mask::rwx $writable_path"
[[ $(restore "$(jq -r .id <<<"$writable")" '{"target":"own_name","stop_containers":true}') == 202 ]] \
    || fail "restore: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == failed*"written by others"* ]] || fail "an archive others may write was restored: $state"
wait_for "the container after the refusal" running 60
[[ $(inside 'cat /data/state') == fourth ]] || fail "the volume changed"
vm "sudo setfacl -m mask::r-x $writable_path"

say "8. One container of two volumes, both at the same hour: stopped until both are done"
[[ $(status PUT "/docker/volumes/$extra/backup-plan" "$plan") == 200 ]] || fail "plan: $(cat "$work/last")"
[[ $(jq -r .hour "$work/last") == "$hour" ]] || fail "not at the same hour"
since=$(vm 'date -u +%Y-%m-%dT%H:%M:%S.%NZ')
[[ $(status POST "/docker/volumes/$extra/backups") == 202 ]] || fail "back up $extra: $(cat "$work/last")"
first_job=$(jq -r .id "$work/last")
[[ $(status POST "/docker/volumes/$volume/backups") == 202 ]] || fail "back up $volume: $(cat "$work/last")"
second_job=$(jq -r .id "$work/last")
job_succeeded "$first_job"
job_succeeded "$second_job"
wait_for "the container after both" running 60
# Started once, after both: a start between the two would be a second one.
# (Polling the jobs cannot tell: the second job starts the container just
# before it is marked done.)
[[ $(starts_since "$since") == 1 ]] || fail "the container was started $(starts_since "$since") times"

say "9. A restore whose unit is stopped midway leaves the old data"
inside 'echo fifth > /extra/state'
latest=$(call GET "/docker/volumes/$extra/backups" | jq -r '[.[] | select(.state == "succeeded")][0].id')
[[ $(restore "$latest" '{"target":"own_name","stop_containers":true}') == 202 ]] \
    || fail "restore: $(cat "$work/last")"
cut=$(jq -r .id "$work/last")
# Stopped as soon as it runs, from outside, as a person or a shutdown would.
unit=zendiloos-volume-restore-$extra.service
vm "for i in \$(seq 1 2400); do sudo systemctl is-active -q $unit && exec sudo systemctl stop $unit; sleep 0.05; done; exit 1" \
    || fail "the restore's unit never ran"
state=$(job_end "$cut")
[[ $state == failed* ]] || fail "a restore cut off succeeded: $state"
wait_for "the container after the cut" running 60
[[ $(inside 'cat /extra/state') == fifth ]] || fail "the old data did not stay"
vm "sudo ls /volume$number/@docker/volumes/$extra" | grep -q '^_data.zendiloos-restore-' \
    || say "   (the unit removed its fresh folder itself)"
[[ $(restore "$latest" '{"target":"own_name","stop_containers":true}') == 202 ]] \
    || fail "restore: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
vm "sudo ls /volume$number/@docker/volumes/$extra" | grep -q '^_data.zendiloos-restore-' \
    && fail "what the cut-off restore left stays beside the data"
wait_for "the container after the restore" running 60

say "10. Leftovers are swept"
hidden="$backups/.$volume-19990101-000000.tar.zst.partial-0123456789abcdef"
vm "echo left | sudo tee $hidden >/dev/null"
back_up_now
vm "sudo test ! -e $hidden" || fail "a hidden file an earlier backup left stays"
fresh="/volume$number/@docker/volumes/$volume/_data.zendiloos-restore-0123456789abcdef"
vm "sudo mkdir -p $fresh/deep && echo left | sudo tee $fresh/deep/file >/dev/null"
vm 'sudo systemctl restart zendiloos-agent.service'
wait_for "the sweep as the agent starts" "vm 'sudo test ! -e $fresh'" 120

say "11. Retention keeps two, and only removes what it recorded"
vm "echo not ours | sudo tee $backups/$volume-19990101-000000.tar.zst >/dev/null"
sleep 2
back_up_now
sleep 2
back_up_now
kept=$(call GET "/docker/volumes/$volume/backups" | jq '[.[] | select(.state == "succeeded")] | length')
[[ $kept == 2 ]] || fail "$kept backups are kept"
vm "sudo test ! -e $writable_path" || fail "an older backup stayed"
# The first archive was changed in step 6: no longer what was recorded, so
# it stays; its manifest was not changed and goes.
vm "sudo test -f $archive" || fail "the archive changed in the share was removed"
vm "sudo test ! -e $manifest" || fail "the first backup's manifest stayed"
# The two kept, the changed one and the one nobody recorded.
[[ $(vm "sudo ls $backups" | grep -c '\.tar\.zst$') == 4 ]] || fail "in the folder: $(vm "sudo ls $backups")"
vm "sudo test -f $backups/$volume-19990101-000000.tar.zst" || fail "a file nobody recorded was removed"

say "12. A volume deleted outside zendiloOS is restored from the Backups page"
[[ $(status PUT "/docker/volumes/$copy/backup-plan" "$plan") == 200 ]] || fail "plan: $(cat "$work/last")"
back_up_now "$copy"
sleep 2
back_up_now "$copy"
vm "sudo docker volume rm $copy" >/dev/null
group=$(backup_group "$copy")
[[ $(jq -r '.exists' <<<"$group") == false ]] || fail "$copy is not marked deleted: $group"
[[ $(jq -r '.plan.paused' <<<"$group") == false ]] || fail "the plan of $copy is gone: $group"
(( $(jq '[.backups[] | select(.state == "succeeded")] | length' <<<"$group") == 2 )) \
    || fail "the backups of $copy: $group"
newest=$(jq -c '.backups[0]' <<<"$group")
older=$(jq -c '.backups[1]' <<<"$group")
[[ $(restore "$(jq -r .id <<<"$newest")" '{"target":"own_name"}') == 202 ]] \
    || fail "restore of the deleted $copy: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
[[ $(in_volume "$copy" 'cat /v/state') == first ]] || fail "$copy was not made again from its backup"
[[ $(backup_group "$copy" | jq -r .exists) == true ]] || fail "$copy is still marked deleted"

say "13. Backups the database forgot are imported from their folder"
forget_record "$(jq -r .id <<<"$newest")"
forget_record "$(jq -r .id <<<"$older")"
[[ $(backup_group "$copy" | jq '.backups | length') == 0 ]] || fail "the records stayed: $(backup_group "$copy")"
newest_path=$(jq -r .archive <<<"$newest")
older_path=$(jq -r .archive <<<"$older")
# Group-writable, as a share's ACL could make it.
vm "sudo chmod g+w /volume$number/$share/$older_path"
import=$(jq -n --argjson share "$share_id" '{share_id:$share,folder:"docker-backups"}')
[[ $(status POST /docker/backups/import "$import") == 202 ]] || fail "import: $(cat "$work/last")"
imported_job=$(jq -r .id "$work/last")
job_succeeded "$imported_job"
output=$(call GET "/jobs/$imported_job" | jq -r '.output[]')
grep -qxF "Recorded $newest_path" <<<"$output" || fail "not recorded: $output"
grep -qxF "Refused $older_path: the archive: writable by others" <<<"$output" \
    || fail "a group-writable archive was not refused: $output"
grep -q "^1 recorded, " <<<"$output" || fail "the import's count: $output"
group=$(backup_group "$copy")
taken=$(jq -c '.backups[0]' <<<"$group")
[[ $(jq -r '.imported' <<<"$taken") == true && $(jq -r .archive <<<"$taken") == "$newest_path" ]] \
    || fail "the imported backup: $group"
[[ $(jq -r .sha256 <<<"$taken") == $(jq -r .sha256 <<<"$newest") ]] || fail "another SHA-256: $taken"
vm "sudo docker run --rm -v $copy:/v alpine:3.20 sh -c 'echo changed > /v/state'"
[[ $(restore "$(jq -r .id <<<"$taken")" '{"target":"own_name"}') == 202 ]] \
    || fail "restore of the imported backup: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
[[ $(in_volume "$copy" 'cat /v/state') == first ]] || fail "the imported backup was not restored"
[[ $(status POST /docker/backups/import "$import") == 202 ]] || fail "import again: $(cat "$work/last")"
again_job=$(jq -r .id "$work/last")
job_succeeded "$again_job"
call GET "/jobs/$again_job" | jq -r '.output[]' | grep -q "^0 recorded, " \
    || fail "recorded again: $(call GET "/jobs/$again_job" | jq -r '.output[]')"
# The plan keeps two: its next two backups leave the imported one alone.
sleep 2
back_up_now "$copy"
sleep 2
back_up_now "$copy"
vm "sudo test -f /volume$number/$share/$newest_path" || fail "retention removed the imported archive"
[[ $(backup_group "$copy" | jq '[.backups[] | select(.imported)] | length') == 1 ]] \
    || fail "the imported backup was forgotten: $(backup_group "$copy")"
[[ $(status DELETE "/docker/backups/$(jq -r .id <<<"$taken")") == 204 ]] || fail "remove: $(cat "$work/last")"
vm "sudo test ! -e /volume$number/$share/$newest_path" || fail "the imported archive stayed"
vm "sudo test -f /volume$number/$share/$older_path" || fail "a file nobody recorded was removed"

say "14. Cleaning up"
[[ $(status DELETE "/docker/volumes/$volume/backup-plan") == 204 ]] || fail "the plan: $(cat "$work/last")"
[[ $(status DELETE "/docker/volumes/$extra/backup-plan") == 204 ]] || fail "the plan: $(cat "$work/last")"
[[ $(status DELETE "/docker/volumes/$copy/backup-plan") == 204 ]] || fail "the plan: $(cat "$work/last")"
remove_project
[[ $(status DELETE "/docker/volumes/$volume") == 204 ]] || fail "removing $volume: $(cat "$work/last")"
[[ $(status DELETE "/docker/volumes/$extra") == 204 ]] || fail "removing $extra: $(cat "$work/last")"
[[ $(status DELETE "/docker/volumes/$copy") == 204 ]] || fail "removing $copy: $(cat "$work/last")"
[[ -z $(volume_json "$volume") ]] || fail "$volume is still listed"

say "All Docker volume tests passed"
