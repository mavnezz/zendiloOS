#!/usr/bin/env bash
# Tests an app given a folder in a share rather than the whole share, end to
# end in a test VM, through the HTTPS API:
#
#   1. the app (Jellyfin, read only) mounts the folder through its pin at
#      /run/zendiloos-app-shares/<app>/<share id>, and sees nothing else of
#      the share
#   2. the folder is swapped for a link to /: a restart by Docker itself
#      still shows the pinned folder, never the host; a start through
#      zendiloOS is refused until the folder is back
#   3. after a reboot the pin is there before Docker starts the app
#   4. removing the app removes its pin and unit
#
# Needs the VM running with the package installed and set up, Docker's data
# on a pool and the network to fetch Jellyfin's image; account and password
# as for vm-storage-test.sh. Runs in the second VM by default (the first
# holds a LAN test). Puts everything back.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
share=appfolders
app=jellyfin
pin=/run/zendiloos-app-shares/$app/data
unit=zendiloos-app-$app-shares.service
container=$app-$app-1
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

job_succeeded() {
    local id=$1 seconds=${2:-900} state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

# What the container shows at /data, one name per line.
inside() { vm "sudo docker exec $container ls -1 /data"; }
running() { [[ $(vm "sudo docker inspect -f '{{.State.Running}}' $container") == true ]]; }

remove_app() {
    if call GET "/apps/installed/$app" | jq -e .id >/dev/null 2>&1; then
        job_succeeded "$(call DELETE "/apps/installed/$app" "{\"confirm\":\"$app\"}" | jq -r .id)"
    fi
}

cleanup() {
    set +e
    remove_app
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

say "Cleaning up an earlier run"
remove_app
for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)"
done

say "1. The app gets the folder Library/Albums, pinned"
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -er .id) || fail "share"
for folder in Library Library/Albums Other; do
    [[ $(status POST "/files/shares/$share_id/folders" "{\"path\":\"$folder\"}") == 201 ]] || fail "folder $folder"
done
volume=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .volume')
path="/volume$volume/$share"
vm "sudo touch $path/Library/Albums/in-albums $path/Other/elsewhere"
# Its permissions (/dev/dri) confirmed as shown: for this digest.
digest=$(call GET /apps/catalog/1/jellyfin | jq -er .digest) || fail "Jellyfin is not in the catalog"
request=$(jq -n --arg folder "$share/Library/Albums" --arg digest "$digest" \
    '{source:1,app:"jellyfin",ports:{},shares:{data:$folder},settings:{},confirm_permissions:true,digest:$digest,pool:null}')
[[ $(status POST /apps/installed "$request") == 202 ]] || fail "install: $(cat "$work/last")"
job_succeeded "$(jq -r .job.id "$work/last")"
call GET "/apps/installed/$app" | jq -e --arg f "$share/Library/Albums" '.shares.data == $f' >/dev/null \
    || fail "the app's share: $(call GET "/apps/installed/$app" | jq -c .shares)"
source=$(vm "sudo docker inspect -f '{{range .Mounts}}{{if eq .Destination \"/data\"}}{{.Source}}{{end}}{{end}}' $container")
[[ $source == "$pin" ]] || fail "the container mounts $source"
vm "sudo findmnt -n $pin" >/dev/null || fail "nothing is mounted at $pin"
[[ $(vm "systemctl is-enabled $unit") == enabled ]] || fail "$unit is not enabled"
wait_for "the container" running 60
[[ $(inside) == in-albums ]] || fail "the container sees: $(inside)"

say "2. The folder becomes a link to /"
vm "sudo mv $path/Library/Albums $path/Library/Albums.real && sudo ln -s / $path/Library/Albums"
vm "sudo docker restart $container" >/dev/null
wait_for "the container again" running 60
seen=$(inside)
[[ $seen == in-albums ]] || fail "after Docker's own restart the container sees: $seen"
[[ $(status POST "/apps/installed/$app/stop") == 204 ]] || fail "stop: $(cat "$work/last")"
[[ $(status POST "/apps/installed/$app/start") == 409 ]] || fail "a start through the link: $(cat "$work/last")"
grep -q "Library/Albums" "$work/last" || fail "the refusal does not name the folder: $(cat "$work/last")"
vm "sudo rm $path/Library/Albums && sudo mv $path/Library/Albums.real $path/Library/Albums"
[[ $(status POST "/apps/installed/$app/start") == 204 ]] || fail "start: $(cat "$work/last")"
wait_for "the container" running 60
[[ $(inside) == in-albums ]] || fail "after the start the container sees: $(inside)"

say "3. A reboot: pinned before Docker starts the app"
vm 'sudo systemctl reboot' || true
sleep 15
wait_for "SSH" "vm true" 300
wait_for "the API" "sign_in" 300
wait_for "the container after the reboot" running 300
[[ $(vm "systemctl is-active $unit") == active ]] || fail "$unit is not active after the reboot"
[[ $(inside) == in-albums ]] || fail "after the reboot the container sees: $(inside)"
pinned=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value $unit")
docker=$(vm "systemctl show -p ActiveEnterTimestampMonotonic --value docker.service")
(( pinned <= docker )) || fail "Docker started before the pin ($docker < $pinned)"

say "4. The app goes, with its pin"
remove_app
vm "test ! -e /run/zendiloos-app-shares/$app" || fail "the pins stayed"
vm "test ! -e /etc/systemd/system/$unit" || fail "the unit stayed"
vm "sudo test -f $path/Library/Albums/in-albums" || fail "the folder's content went"

say "All app folder tests passed"
