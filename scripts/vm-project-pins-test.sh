#!/usr/bin/env bash
# Tests that an administrator's Compose project runs as it was confirmed,
# end to end in a test VM, through the HTTPS API:
#
#   1. a project in a share comes up from what was confirmed, kept by root;
#      its binds into the share (a folder Docker would make, a file) are
#      pinned below /run/zendiloos-project-shares/<project>
#   2. the file in the share is changed by someone who may write the share
#      (privileged, the host's root): a redeploy still runs what was
#      confirmed, and the editor shows that
#   3. the bound folder becomes a link to /: a restart by Docker itself
#      still shows the pinned folder, a redeploy is refused until the folder
#      is back
#   4. the confirmed file binds another folder: the container is made again
#      on another pin, and back
#   5. after a reboot the pins are there before Docker starts the project
#   6. removing the project removes what deploying it kept; the share's
#      data stays
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
share=projectpins
project=pinstest
pins=/run/zendiloos-project-shares/$project
unit=zendiloos-project-$project-shares.service
kept=/etc/zendiloos/projects/$project.json
container=$project-app-1
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
# What the container mounts at $1.
mounted() { vm "sudo docker inspect -f '{{range .Mounts}}{{if eq .Destination \"$1\"}}{{.Source}}{{end}}{{end}}' $container"; }
inside() { vm "sudo docker exec $container ls -1 /data"; }

remove_project() {
    if call GET "/docker/projects/$project" | jq -e .name >/dev/null 2>&1; then
        job_succeeded "$(call DELETE "/docker/projects/$project" | jq -r .id)"
    fi
}

cleanup() {
    set +e
    remove_project
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
remove_project
for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
done

say "1. The project comes up as confirmed, its binds pinned"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
[[ $(status POST "/files/shares/$share_id/folders" '{"path":"docker"}') == 201 ]] || fail "folder docker"
[[ $(status POST "/files/shares/$share_id/folders" "{\"path\":\"docker/$project\"}") == 201 ]] || fail "folder"
volume=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .volume')
folder="/volume$volume/$share/docker/$project"
vm "echo confirmed | sudo tee $folder/settings.conf >/dev/null"
compose=$'services:\n  app:\n    image: alpine:3.20\n    restart: unless-stopped\n    command: ["sleep", "infinity"]\n    volumes:\n      - ./data:/data\n      - ./settings.conf:/settings.conf:ro\n'
request=$(jq -n --arg name "$project" --argjson share "$share_id" --arg folder "docker/$project" --arg compose "$compose" \
    '{name:$name,share_id:$share,folder:$folder,compose:$compose,start:true,accept_risks:false}')
[[ $(status POST /docker/projects "$request") == 201 ]] || fail "create: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
wait_for "the container" running 60
[[ $(vm "sudo stat -c %a $kept") == 600 ]] || fail "$kept is not root's alone"
first_pin=$(mounted /data)
[[ $first_pin == "$pins"/* ]] || fail "/data comes from $first_pin"
[[ $(mounted /settings.conf) == "$pins"/* && $(mounted /settings.conf) != "$first_pin" ]] \
    || fail "/settings.conf comes from $(mounted /settings.conf)"
[[ $(vm "systemctl is-enabled $unit") == enabled ]] || fail "$unit is not enabled"
vm "sudo test -d $folder/data" || fail "the missing folder was not made"
vm "sudo docker exec $container touch /data/from-container"
vm "sudo test -f $folder/data/from-container" || fail "the pin is not the share's folder"
[[ $(vm "sudo docker exec $container cat /settings.conf") == confirmed ]] || fail "the file bind"

say "2. The file in the share is changed: what was confirmed still runs"
vm "printf '    privileged: true\n' | sudo tee -a $folder/compose.yaml >/dev/null"
vm "sudo sed -i 's|      - ./data:/data|      - ./data:/data\n      - /:/host|' $folder/compose.yaml"
vm "sudo grep -q privileged $folder/compose.yaml" || fail "the test could not change the file"
[[ $(status POST "/docker/projects/$project/redeploy") == 202 ]] || fail "redeploy: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
wait_for "the container" running 60
[[ $(vm "sudo docker inspect -f '{{.HostConfig.Privileged}}' $container") == false ]] || fail "it runs privileged"
[[ -z $(mounted /host) ]] || fail "it mounts the host's root"
[[ $(call GET "/docker/projects/$project/compose" | jq -r '.files[0].content') == "${compose%$'\n'}" ]] \
    || fail "the editor does not show what was confirmed"

say "3. The bound folder becomes a link to /"
vm "sudo mv $folder/data $folder/data.real && sudo ln -s / $folder/data"
vm "sudo docker restart $container" >/dev/null
wait_for "the container again" running 60
[[ $(inside) == from-container ]] || fail "after Docker's own restart the container sees: $(inside)"
[[ $(status POST "/docker/projects/$project/redeploy") == 202 ]] || fail "redeploy: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == failed*link* ]] || fail "a redeploy through the link: $state"
vm "sudo rm $folder/data && sudo mv $folder/data.real $folder/data"
[[ $(status POST "/docker/projects/$project/redeploy") == 202 ]] || fail "redeploy: $(cat "$work/last")"
job_succeeded "$(jq -r .id "$work/last")"
wait_for "the container" running 60
[[ $(inside) == from-container ]] || fail "after the redeploy the container sees: $(inside)"

say "4. Another folder bound: the container is made again"
change() {
    [[ $(status PUT "/docker/projects/$project/compose" "$(jq -n --arg c "$1" '{compose:$c}')") == 202 ]] \
        || fail "change: $(cat "$work/last")"
    job_succeeded "$(jq -r .id "$work/last")"
    wait_for "the container" running 60
}
change "${compose/.\/data:/.\/other:}"
[[ $(mounted /data) == "$pins"/* && $(mounted /data) != "$first_pin" ]] || fail "/data still from $(mounted /data)"
[[ -z $(inside) ]] || fail "the container still sees the old folder: $(inside)"
vm "sudo test -d $folder/other" || fail "the new folder was not made"
change "$compose"
[[ $(mounted /data) == "$first_pin" ]] || fail "back: /data from $(mounted /data)"
[[ $(inside) == from-container ]] || fail "back: the container sees $(inside)"

[[ $(status POST "/shares/$share_id/delete" "{\"confirm\":\"$share\"}") == 409 ]] \
    || fail "the share a project lives in went: $(cat "$work/last")"
jq -e '.code == "share_used_by_projects"' "$work/last" >/dev/null || fail "the refusal: $(cat "$work/last")"

say "5. A reboot: pinned before Docker starts the project"
vm 'sudo systemctl reboot' || true
sleep 15
wait_for "SSH" "vm true" 300
wait_for "the API" "sign_in" 300
wait_for "the container after the reboot" running 300
[[ $(vm "systemctl is-active $unit") == active ]] || fail "$unit is not active after the reboot"
[[ $(inside) == from-container ]] || fail "after the reboot the container sees: $(inside)"
pinned=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value $unit")
docker=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value docker.service")
(( pinned <= docker )) || fail "Docker started before the pins ($docker < $pinned)"

say "6. The project goes, with what it kept"
remove_project
vm "sudo test ! -e $kept" || fail "$kept stayed"
vm "test ! -e $pins" || fail "the pins stayed"
vm "test ! -e /etc/systemd/system/$unit" || fail "the unit stayed"
vm "sudo test -f $folder/data/from-container" || fail "the share's data went"

say "All project pin tests passed"
