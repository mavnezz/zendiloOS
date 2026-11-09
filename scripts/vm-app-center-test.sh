#!/usr/bin/env bash
# Tests the App Center end to end in a test VM, through the HTTPS API, with
# docker inside the VM: an app of the catalog becomes an administrator's own
# Docker project.
#
#   1. Lidarr's template is filled in: a folder of a share, a port;
#      the values stand in the file itself, and it kept to the catalog's
#      policy as Debian's Compose reads it (with the catalog's stand-ins)
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
#   7. a catalog of one's own, served over https inside the VM by a CA the
#      VM trusts for the test, offers an app with `privileged: true` and one
#      with a named volume binding / (`o: rbind`): the index offers both,
#      as it sees only the files as written, and filling either in is
#      refused (`against_catalog_policy`) once Compose has read it
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
# The catalog of one's own (step 7): served from this folder in the VM, on
# this port, with a certificate of a CA trusted for the test only.
catalog=/run/zendiloos-test-catalog
catalog_port=18643
catalog_unit=zendiloos-test-catalog
catalog_ca=/usr/local/share/ca-certificates/zendiloos-test-catalog.crt
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

sign_in() {
    curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf"
}

# zendiloosd reads the trusted CAs when it starts.
restart_zendiloosd() {
    vm "sudo systemctl restart zendiloosd"
    wait_for "zendiloosd" "curl -skf $api/health" 120
    sign_in || fail "cannot sign in again"
}

# The catalog of one's own goes, and the CA with it; true if there was one.
remove_catalog() {
    local id
    for id in $(call GET /apps/sources | jq -r --arg a "https://127.0.0.1:$catalog_port/v1/index.json" \
        '.[] | select(.address == $a) | .id'); do
        call DELETE "/apps/sources/$id" >/dev/null
    done
    vm "sudo systemctl stop $catalog_unit 2>/dev/null; sudo systemctl reset-failed $catalog_unit 2>/dev/null; sudo rm -rf $catalog
        if [ -e $catalog_ca ]; then sudo rm $catalog_ca && sudo update-ca-certificates --fresh >/dev/null; exit 0; fi
        exit 1"
}

cleanup() {
    set +e
    remove_project
    delete_shares
    remove_catalog && restart_zendiloosd
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
sign_in || fail "cannot sign in"
me=$(call GET /auth/session | jq -er .account.id)
home=$(call GET /docker/data | jq -er .current.pool) || fail "Docker's data is not on a pool"
volume=$(call GET /storage/pools | jq -r --arg p "$home" '.[] | select(.name == $p) | .volume')

say "Cleaning up an earlier run"
remove_project
delete_shares
if remove_catalog; then restart_zendiloosd; fi

say "1. Lidarr's template, filled in"
source_id=$(call GET /apps/catalog | jq -er '[.[] | select(.id == "lidarr" and .official)][0].source_id') \
    || fail "the catalog has no Lidarr"
access='[{kind:"account",id:$me,access:"read_write"}]'
share_id=$(call POST /shares "$(jq -n --arg n "$share" --arg pool "$home" --argjson me "$me" \
    "{name:\$n,pool:\$pool,access:$access}")" | jq -er .id) || fail "share"
call POST /shares "$(jq -n --arg n "$media" --arg pool "$home" --argjson me "$me" \
    "{name:\$n,pool:\$pool,access:$access}")" | jq -er .id >/dev/null || fail "media share"
vm "sudo mkdir -p /volume$volume/$media/Music && echo tune | sudo tee /volume$volume/$media/Music/song.txt >/dev/null"
fill=$(jq -n --arg n "$project" --argjson s "$share_id" --arg m "$media/Music" --argjson p "$port" \
    '{name:$n,share_id:$s,folder:"docker/lidarr",shares:{data:$m},ports:{web:$p}}')
filled=$(call POST "/apps/catalog/$source_id/lidarr/compose" "$fill" | jq -er .compose) \
    || fail "cannot fill the template: $(call POST "/apps/catalog/$source_id/lidarr/compose" "$fill")"
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

say "7. A catalog of one's own whose apps break the policy"
# An index entry as a catalog publishes it, with `compose` as its file.
probe() {
    jq -n --arg id "$1" --arg compose "$2" '{
        id: $id, name: "Policy probe", tagline: "Shows what the catalog policy refuses",
        description: "Not an app to run.\n", categories: ["monitoring"],
        developer: "zendiloOS tests", website: "https://example.org", source: "https://example.org/source",
        license: "MIT", maintainers: ["mavnezz"], architectures: ["amd64", "arm64"],
        web: {port: "web", path: "/", scheme: "http"},
        ports: [{id: "web", label: "Web interface", default: 18690}],
        compose: $compose, icon: ("icons/" + $id + ".png"),
        digest: "sha256:2cbf42f705a743b98a7c563720a8b5f8349bf6539b3f1985aa55ed73f5251fbf"
    }'
}
plain_probe=$'services:\n  probe:\n    image: alpine:3.20\n    restart: unless-stopped\n    environment:\n      TZ: ${TZ}\n    ports:\n      - ${PORT_WEB}:8080\n    volumes:\n      - ${APP_DATA}/data:/data\n'
privileged_probe=${plain_probe/restart: unless-stopped/restart: unless-stopped$'\n'    privileged: true}
rbind_probe=$plain_probe$'      - host:/host\nvolumes:\n  host:\n    driver_opts:\n      type: none\n      o: rbind\n      device: /\n'
index=$(jq -n --argjson a "$(probe probe-privileged "$privileged_probe")" --argjson b "$(probe probe-rbind "$rbind_probe")" \
    '{format: 1, apps: [$a, $b]}')
# A CA of the test's own, trusted by the VM until the test ends, signs the
# certificate of a server for 127.0.0.1 that serves the index.
vm "set -e
    sudo mkdir -p $catalog/www/v1
    cd $catalog
    sudo openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
        -subj '/CN=zendiloOS test catalog' -addext basicConstraints=critical,CA:TRUE \
        -addext keyUsage=critical,keyCertSign -keyout ca.key -out ca.pem 2>/dev/null
    sudo openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=127.0.0.1 \
        -keyout server.key -out server.csr 2>/dev/null
    printf 'subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' | sudo tee server.ext >/dev/null
    sudo openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 1 \
        -extfile server.ext -out server.pem 2>/dev/null
    sudo cp ca.pem $catalog_ca
    sudo update-ca-certificates >/dev/null" || fail "cannot make the catalog's certificates"
vm "sudo tee $catalog/www/v1/index.json >/dev/null" <<<"$index"
vm "sudo systemd-run --quiet --unit $catalog_unit -p WorkingDirectory=$catalog/www -p CollectMode=inactive-or-failed \
    openssl s_server -quiet -accept 127.0.0.1:$catalog_port -cert $catalog/server.pem -key $catalog/server.key -WWW" \
    || fail "cannot serve the catalog"
wait_for "the catalog's server" "vm 'curl -sf https://127.0.0.1:$catalog_port/v1/index.json'" 30
restart_zendiloosd
[[ $(status POST /apps/sources "{\"address\":\"https://127.0.0.1:$catalog_port/v1/index.json\"}") == 201 ]] \
    || fail "the catalog was not added: $(cat "$work/last")"
own=$(jq -er .id "$work/last")
jq -e '.skipped == 0 and .refresh_error == null' "$work/last" >/dev/null || fail "its index: $(cat "$work/last")"
# The index sees the files as written only: both are offered.
call GET /apps/catalog | jq -e --argjson s "$own" '[.[] | select(.source_id == $s) | .id] | sort == ["probe-privileged", "probe-rbind"]' \
    >/dev/null || fail "not offered: $(call GET /apps/catalog | jq -c --argjson s "$own" '[.[] | select(.source_id == $s)]')"
refused() {
    local body
    body=$(jq -n --argjson s "$share_id" '{name:"zprobe",share_id:$s,folder:"docker/zprobe",ports:{web:18690}}')
    [[ $(status POST "/apps/catalog/$own/$1/compose" "$body") == 422 ]] || fail "$1 was filled in: $(cat "$work/last")"
    jq -e --arg w "$2" '.code == "against_catalog_policy" and (.detail | contains($w))' "$work/last" >/dev/null \
        || fail "$1: $(cat "$work/last")"
}
refused probe-privileged "privileged"
refused probe-rbind "volumes.host"
remove_catalog || fail "the catalog was not there to remove"
restart_zendiloosd

say "All App Center tests passed"
