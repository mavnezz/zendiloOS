#!/usr/bin/env bash
# Tests the App Center end to end in a test VM, through the HTTPS API, with
# docker inside the VM: an app of the catalog becomes an administrator's own
# Docker project.
#
#   1. Lidarr's template is filled in: a folder of a share, a port;
#      the values stand in the file itself
#   2. the file is written into a folder of a share and deployed as a
#      project of one's own: the file and the data folder are the
#      administrator's, the container runs as the administrator in the
#      group zendiloos-files (which every share's folder lets read and
#      write) and sees the folder
#   3. the App Center finds the project again, with the web address, which
#      answers; the name is not handed out twice
#   4. the file is changed in the Docker app (another port) and deployed
#      again: the App Center's address follows
#   5. apps installed the earlier way are still listed, stop and start
#   6. the project goes, its folder stays
#
# Needs the VM running with the package installed and set up, Docker on a
# pool and the network to fetch Lidarr's image; account and password as
# for vm-storage-test.sh. Runs in the second VM by default (the first holds
# a LAN test). Puts everything back.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
share=apptest
media=apptestmedia
project=zapptest
port=18686
moved=18687
work=$(mktemp -d)

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
    local what=$1 check=$2 seconds=${3:-120}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

job_succeeded() {
    local id=$1 seconds=${2:-900} state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

delete_shares() {
    local name id
    for name in "$share" "$media"; do
        for id in $(call GET /shares | jq -r --arg s "$name" '.[] | select(.name == $s) | .id'); do
            job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$name\"}" | jq -r .id)"
        done
    done
}

remove_project() {
    if call GET /docker/projects | jq -e --arg p "$project" '.[] | select(.name == $p)' >/dev/null; then
        job_succeeded "$(call DELETE "/docker/projects/$project" | jq -r .id)"
    fi
}

cleanup() {
    set +e
    remove_project
    delete_shares
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
me=$(call GET /auth/session | jq -er .account.id)
home=$(call GET /docker/data | jq -er .current.pool) || fail "Docker's data is not on a pool"
volume=$(call GET /storage/pools | jq -r --arg p "$home" '.[] | select(.name == $p) | .volume')

say "Cleaning up an earlier run"
remove_project
delete_shares

say "1. Lidarr's template, filled in"
source_id=$(call GET /apps/catalog | jq -er '[.[] | select(.id == "lidarr" and .official)][0].source_id') \
    || fail "the catalog has no Lidarr"
access='[{kind:"account",id:$me,access:"read_write"}]'
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$home" --argjson me "$me" \
    "{name:\$n,pool:\$pool,access:$access}")" | jq -er .id) || fail "share"
call POST /shares "$(jq -n --arg n "$media" --arg pool "$home" --argjson me "$me" \
    "{name:\$n,pool:\$pool,access:$access}")" | jq -er .id >/dev/null || fail "media share"
vm "sudo mkdir -p /volume$volume/$media/Music && echo tune | sudo tee /volume$volume/$media/Music/song.txt >/dev/null"
filled=$(call POST "/apps/catalog/$source_id/lidarr/compose" \
    "$(jq -n --arg m "$media/Music" --argjson p "$port" '{shares:{data:$m},ports:{web:$p}}')" | jq -er .compose) \
    || fail "cannot fill the template: $(call POST "/apps/catalog/$source_id/lidarr/compose" '{}')"
# The app runs as the administrator, in the group of the shares' files.
uid=$(vm "id -u $user") gid=$(vm "getent group zendiloos-files | cut -d: -f3")
[[ $gid =~ ^[0-9]+$ ]] || fail "no group zendiloos-files: $gid"
for line in "$port:8686" "./config:/config" "/volume$volume/$media/Music:/data" "PUID: \"$uid\"" "PGID: \"$gid\""; do
    grep -qF -- "$line" <<<"$filled" || fail "the filled file lacks $line: $filled"
done
! grep -q '\${' <<<"$filled" || fail "a variable is left: $filled"

say "2. A project of one's own"
new=$(jq -n --arg n "$project" --argjson s "$share_id" --arg c "$filled" --argjson src "$source_id" \
    '{name:$n,share_id:$s,folder:"docker/lidarr",compose:$c,start:true,
      template:{source_id:$src,app:"lidarr"}}')
[[ $(status POST /docker/projects "$new") == 201 ]] || fail "create: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
folder="/volume$volume/$share/docker/lidarr"
for entry in compose.yaml config; do
    owner=$(vm "sudo stat -c %U $folder/$entry")
    [[ $owner == "$user" ]] || fail "$entry belongs to $owner"
done
container=$(vm "sudo docker ps -q --filter label=com.docker.compose.project=$project")
[[ -n $container ]] || fail "no container runs"
environment=$(vm "sudo docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' $container")
grep -qx "PUID=$uid" <<<"$environment" || fail "the container does not run as the administrator: $environment"
grep -qx "PGID=$gid" <<<"$environment" || fail "the container does not run in the group zendiloos-files: $environment"
[[ $(vm "sudo docker exec $container cat /data/song.txt") == tune ]] || fail "the share's folder is not mounted"
[[ $(vm "sudo docker exec $container stat -c %u /config") == "$uid" ]] || fail "/config is not the administrator's"
call GET "/docker/projects/$project" \
    | jq -e --argjson src "$source_id" '.kind == "own" and .template.app == "lidarr" and .template.source_id == $src' \
    >/dev/null || fail "project: $(call GET "/docker/projects/$project")"

say "3. The App Center finds it again"
found() { call GET /apps/projects | jq -e --arg p "$project" --argjson port "$1" \
    '.[] | select(.project == $p and .id == "lidarr" and .web.port == $port)'; }
wait_for "the web address" "found $port" 60
wait_for "Lidarr to answer" "vm \"curl -sf -o /dev/null http://127.0.0.1:$port/\"" 120
[[ $(status POST /docker/projects "$new") == 409 ]] || fail "the name was handed out twice: $(cat "$work/last")"

say "4. Changed in the Docker app"
changed=$(call GET "/docker/projects/$project/compose" | jq -er '.files[0].content' | sed "s/$port:8686/$moved:8686/")
grep -qF "$moved:8686" <<<"$changed" || fail "nothing to change"
job_succeeded "$(call PUT "/docker/projects/$project/compose" "$(jq -n --arg c "$changed" '{compose:$c}')" | jq -r .id)"
wait_for "the address to follow" "found $moved" 60
wait_for "Lidarr on the new port" "vm \"curl -sf -o /dev/null http://127.0.0.1:$moved/\"" 120
vm "sudo grep -qF '$moved:8686' $folder/compose.yaml" || fail "the share's copy was not written"

say "5. Apps installed the earlier way"
earlier=$(call GET /apps/installed | jq -r '[.[] | select(.state == "installed")][0].id // empty')
if [[ -n $earlier ]]; then
    [[ $(status POST "/apps/installed/$earlier/stop") == 204 ]] || fail "stop $earlier: $(cat "$work/last")"
    wait_for "$earlier to stop" "call GET /apps/installed | jq -e --arg a $earlier '.[] | select(.id == \$a and .run_state == \"stopped\")'" 60
    [[ $(status POST "/apps/installed/$earlier/start") == 204 ]] || fail "start $earlier: $(cat "$work/last")"
    wait_for "$earlier to run" "call GET /apps/installed | jq -e --arg a $earlier '.[] | select(.id == \$a and .run_state == \"running\")'" 120
else
    echo "   none installed; skipped"
fi

say "6. The project goes, its folder stays"
remove_project
vm "sudo test -f $folder/compose.yaml && sudo test -d $folder/config" || fail "the folder went with the project"
call GET /apps/projects | jq -e --arg p "$project" 'map(select(.project == $p)) | length == 0' >/dev/null \
    || fail "the App Center still lists it"

say "All App Center tests passed"
