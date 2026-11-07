#!/usr/bin/env bash
# Tests the Docker app end to end in a test VM, through the HTTPS API, with
# docker and zfs inside the VM:
#
#   1. a project's Compose file is checked: what reaches beyond the
#      containers needs consent; a plain one is written into a share and
#      deployed, as the administrator who made it
#   2. the project stops, starts, writes logs, changes and is deployed again;
#      one container shows its details and processes, and a terminal in it
#      answers, follows a new size and ends with the socket
#   3. a network comes and goes; registries are asked for newer images
#   4. the project is removed; its folder stays
#   5. Docker's data moves to another pool and back, and the app on it runs
#      again each time
#
# Needs the VM running with the package installed and set up, Docker on a
# pool, two free disks and an image `alpine:3.20` (or the network to fetch
# it); account and password as for vm-storage-test.sh. Runs in the second
# VM by default (the first holds a LAN test). Puts everything back.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
share=dockertest
spare=dockermove
project=ztest
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
    local id=$1 seconds=${2:-600} state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

kuma_runs() { [[ $(vm "sudo docker inspect -f '{{.State.Running}}' uptime-kuma-uptime-kuma-1") == true ]]; }

cleanup() {
    set +e
    call DELETE "/docker/projects/$project" >/dev/null
    call DELETE /docker/networks/ztest_net >/dev/null
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" >/dev/null
    done
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
me=$(call GET /auth/session | jq -er .account.id)
call GET /docker/overview | jq -e '.containers >= 1' >/dev/null || fail "no overview: $(call GET /docker/overview)"
home=$(call GET /docker/data | jq -er .current.pool) || fail "Docker's data is not on a pool"
vm 'sudo docker image inspect alpine:3.20 >/dev/null 2>&1 || sudo docker pull -q alpine:3.20' >/dev/null

say "Cleaning up an earlier run"
for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
done
if call GET /storage/pools | jq -e --arg p "$spare" '.[] | select(.name == $p)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$spare/destroy" "{\"confirm\":\"$spare\"}" | jq -r .id)"
fi

say "1. A project: consent first, then written and deployed"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$home" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
call GET /docker/places | jq -e --argjson id "$share_id" '.[] | select(.share_id == $id)' >/dev/null \
    || fail "the share is no place for projects"
risky='services:\n  app:\n    image: alpine:3.20\n    privileged: true\n    volumes:\n      - /var/run/docker.sock:/var/run/docker.sock\n'
plain='services:\n  app:\n    image: alpine:3.20\n    command: ["sh", "-c", "echo first; sleep 3600"]\n    volumes:\n      - ./data:/data\n'
new() {
    jq -n --arg n "$project" --argjson s "$share_id" --arg c "$(printf "$1")" --argjson accept "$2" \
        '{name:$n,share_id:$s,folder:"docker/ztest",compose:$c,start:true,accept_risks:$accept}'
}
kinds=$(call POST /docker/compose/check "$(jq -n --arg n "$project" --argjson s "$share_id" --arg c "$(printf "$risky")" \
    '{name:$n,share_id:$s,folder:"docker/ztest",compose:$c}')" | jq -c '[.risks[].kind] | sort')
[[ $kinds == '["docker_socket","privileged"]' ]] || fail "risks: $kinds"
[[ $(status POST /docker/projects "$(new "$risky" false)") == 409 ]] || fail "risky without consent: $(cat "$work/last")"
jq -e '.code == "compose_risks"' "$work/last" >/dev/null || fail "$(cat "$work/last")"
[[ $(status POST /docker/projects "$(new 'services:\n  app: [' false)") == 409 ]] || fail "a broken file"
[[ $(status POST /docker/projects "$(new "$plain" false)") == 201 ]] || fail "create: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
volume=$(call GET /storage/pools | jq -r --arg p "$home" '.[] | select(.name == $p) | .volume')
folder="/volume$volume/$share/docker/ztest"
vm "sudo test -f $folder/compose.yaml && sudo test -d $folder/data" || fail "no file or data folder in $folder"
owner=$(vm "sudo stat -c %U $folder/compose.yaml")
[[ $owner == "$user" ]] || fail "the file belongs to $owner"
call GET "/docker/projects/$project" | jq -e '.kind == "own" and .running == 1' >/dev/null \
    || fail "project: $(call GET "/docker/projects/$project")"

say "2. Stop, start, logs, change"
[[ $(status POST "/docker/projects/$project/stop") == 204 ]] || fail "stop"
call GET "/docker/projects/$project" | jq -e '.running == 0' >/dev/null || fail "still running"
[[ $(status POST "/docker/projects/$project/start") == 204 ]] || fail "start"
wait_for "the log" "call GET '/docker/projects/$project/logs' | jq -er .text | grep -q first" 30
changed='services:\n  app:\n    image: alpine:3.20\n    command: ["sh", "-c", "echo second; sleep 3600"]\n'
[[ $(status PUT "/docker/projects/$project/compose" "$(jq -n --arg c "$(printf "$changed")" '{compose:$c}')") == 202 ]] \
    || fail "change: $(cat "$work/last")"
change_job=$(jq -r .id "$work/last")
job_succeeded "$change_job"
# What Compose wrote while it deployed stays with the job, as text.
call GET "/jobs/$change_job" | jq -e '.output | map(select(test("Container .*(Started|Recreated|Running)"))) | length > 0' \
    >/dev/null || fail "the deploy kept no output: $(call GET "/jobs/$change_job" | jq -c .output)"
call GET "/jobs/$change_job" | jq -e '.output | all(test("\u001b") | not)' >/dev/null || fail "escapes in the output"
wait_for "the new log" "call GET '/docker/projects/$project/logs' | jq -er .text | grep -q second" 30
call GET "/docker/projects/$project/compose" | jq -e '.editable and (.files[0].content | contains("second"))' >/dev/null \
    || fail "the file was not changed"
call GET "/docker/projects/$project" | jq -e --arg f "$share/docker/ztest/compose.yaml" '.compose_file == $f' >/dev/null \
    || fail "where the file is: $(call GET "/docker/projects/$project" | jq -r .compose_file)"
# A redeploy pulls only when asked; pulling, Compose says so.
[[ $(status POST "/docker/projects/$project/redeploy" '{"pull":true}') == 202 ]] || fail "redeploy: $(cat "$work/last")"
pull_job=$(jq -r .id "$work/last")
job_succeeded "$pull_job"
call GET "/jobs/$pull_job" | jq -e '.output | map(select(test("[Pp]ull"))) | length > 0' >/dev/null \
    || fail "no pull in the output: $(call GET "/jobs/$pull_job" | jq -c .output)"

say "2b. One container: its details, processes and a terminal"
box=$(call GET "/docker/projects/$project" | jq -er '.containers[0].name') || fail "no container"
call GET "/docker/containers/$box" | jq -e '.mounts | map(select(.destination == "/data")) | length == 1' >/dev/null \
    || fail "the container's details: $(call GET "/docker/containers/$box" | jq -c .)"
call GET "/docker/containers/$box/processes" | jq -e '[.. | strings | select(test("sleep 3600"))] | length > 0' >/dev/null \
    || fail "its processes: $(call GET "/docker/containers/$box/processes" | jq -c .)"
session=$(awk '$6 ~ /zendiloos_session$/ { print $6 "=" $7 }' "$work/admin.jar")
wss="wss://${base#https://}/api/v1/docker/containers/$box/terminal?command=/bin/sh&columns=80&rows=24"
NODE_TLS_REJECT_UNAUTHORIZED=0 node "$root/scripts/vm-terminal-probe.mjs" "$wss" "$session" "$base" >"$work/terminal" 2>&1 \
    || fail "the terminal: $(cat "$work/terminal")"
# Closing it hangs up the terminal, which ends what ran in it.
wait_for "the terminal's program to end" "! vm 'pgrep -f \"docker exec .*$box\"'" 30
call GET '/logs/audit' | jq -e '[.. | strings | select(test("container_terminal"))] | length > 0' >/dev/null \
    || fail "no audit entry for the terminal"

say "3. A network; newer images"
[[ $(status POST /docker/networks '{"name":"ztest_net","subnet":"172.31.250.0/24","gateway":"172.31.250.1"}') == 201 ]] \
    || fail "network: $(cat "$work/last")"
[[ $(vm "sudo docker network inspect -f '{{(index .IPAM.Config 0).Subnet}}' ztest_net") == 172.31.250.0/24 ]] || fail "subnet"
[[ $(status DELETE /docker/networks/ztest_net) == 204 ]] || fail "network removal"
[[ $(status DELETE /docker/networks/bridge) == 409 ]] || fail "Docker's own network went"
call POST /docker/updates/check | jq -e '.images[] | select(.reference == "alpine:3.20")' >/dev/null \
    || fail "alpine not checked"

say "4. The project goes, its folder stays"
[[ $(status DELETE "/docker/projects/$project") == 202 ]] || fail "delete"
job_succeeded "$(jq -r .id "$work/last")"
[[ -z $(vm "sudo docker ps -aq --filter label=com.docker.compose.project=$project") ]] || fail "containers left"
vm "sudo test -f $folder/compose.yaml" || fail "the folder went"
[[ $(status GET "/docker/projects/$project") == 404 ]] || fail "still listed"

say "5. Docker's data to another pool and back"
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$spare\",\"layout\":\"mirror\",\"disks\":$free}")
job_succeeded "$(call POST /storage/pools "{\"token\":\"$(jq -er .token <<<"$plan")\",\"confirm\":\"$spare\"}" | jq -r .id)"
images_before=$(vm 'sudo docker images -q | sort | tr "\n" " "')
[[ $(status POST /docker/data/move "{\"pool\":\"$spare\"}") == 202 ]] || fail "move: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")" 1800
spare_volume=$(call GET /storage/pools | jq -r --arg p "$spare" '.[] | select(.name == $p) | .volume')
[[ $(vm "sudo docker info -f '{{.DockerRootDir}}'") == "/volume$spare_volume/@docker" ]] || fail "data root"
[[ $(vm 'sudo docker images -q | sort | tr "\n" " "') == "$images_before" ]] || fail "images lost"
wait_for "uptime-kuma on $spare" kuma_runs 120
[[ -z $(vm "sudo zfs list -H -o name $home/zendiloos.docker 2>/dev/null") ]] || fail "the old dataset stayed"
[[ $(status POST /docker/data/move "{\"pool\":\"$home\"}") == 202 ]] || fail "move back"
job_succeeded "$(jq -r .id "$work/last")" 1800
[[ $(vm "sudo docker info -f '{{.DockerRootDir}}'") == "/volume$volume/@docker" ]] || fail "data root back"
wait_for "uptime-kuma on $home" kuma_runs 120
job_succeeded "$(call POST "/storage/pools/$spare/destroy" "{\"confirm\":\"$spare\"}" | jq -r .id)"

say "All Docker tests passed"
