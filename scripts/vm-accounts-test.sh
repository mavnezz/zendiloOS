#!/usr/bin/env bash
# Tests accounts, groups and personal folders end to end in the test VM,
# through the HTTPS API and with smbclient inside the VM:
#
#   1. the password policy: a rule set by an administrator refuses a
#      password, common passwords are always refused
#   2. a group denied a share keeps its members out, over SMB and in the
#      file system, even a member with a rule of their own; members who
#      join and leave the group follow
#   3. personal folders: on a pool, given to one account with a quota,
#      reachable over SMB as the share home and in the file manager by that
#      account only
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Uses two free disks of the VM for a
# pool of its own and leaves pools in use alone.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=accounttest
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call_as() {
    local session=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk -b "$work/$session.jar" -c "$work/$session.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/$session.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}
call() { call_as admin "$@"; }
status() { # method path [body]: the status code, the body in $work/body
    local args=(-sk -o "$work/body" -w '%{http_code}' -b "$work/admin.jar" -X "$1" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf")")
    [[ -n ${3:-} ]] && args+=(-H 'Content-Type: application/json' -d "$3")
    curl "${args[@]}" "$api$2"
}

sign_in() { # session name password
    curl -sk -c "$work/$1.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$2" --arg password "$3" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/$1.csrf" || fail "cannot sign in as $2"
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
    local id=$1 state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 300
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

smb() { # share account password command…
    local share=$1 account=$2 secret=$3; shift 3
    vm "cd /tmp && smbclient //localhost/$share -U '$account%$secret' -c '$*' 2>&1" || true
}
smb_ok() { local out; out=$(smb "$@"); ! grep -q NT_STATUS <<<"$out" || fail "SMB to $1 as $2: $out"; }
smb_refused() { local out; out=$(smb "$@"); grep -q NT_STATUS <<<"$out" || fail "SMB to $1 as $2 connected: $out"; }
acl_of() { vm "sudo getfacl -cp '$1' 2>/dev/null"; }

carl_secret='pictures 4 the album'
dora_secret='stories 9 for reading'

say "Clients in the VM"
vm "command -v smbclient >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient >/dev/null"
vm "command -v smbclient" >/dev/null || fail "smbclient is missing in the VM"

say "Signing in as $user"
sign_in admin "$user" "$password"

say "Cleaning up an earlier run"
call PUT /homes '{"pool":null}' >/dev/null
call PUT /password-policy '{"min_length":10,"mixed_case":false,"digit":false,"symbol":false,"max_age_days":null}' >/dev/null
for id in $(call GET /shares | jq -r '.[] | select(.name == "albums") | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" '{"confirm":"albums"}' | jq -r .id)"
done
for id in $(call GET /groups | jq -r '.[] | select(.name == "guests") | .id'); do
    call DELETE "/groups/$id" >/dev/null
done
for name in carl dora; do
    id=$(call GET /accounts | jq -r --arg name "$name" '.[] | select(.name == $name) | .id')
    [[ -n $id ]] && call DELETE "/accounts/$id" >/dev/null
done
if call GET /storage/pools | jq -e --arg pool "$pool" '.[] | select(.name == $pool)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi

say "1. Password rules"
call PUT /password-policy '{"min_length":12,"mixed_case":false,"digit":true,"symbol":false,"max_age_days":null}' \
    | jq -e '.min_length == 12 and .digit' >/dev/null || fail "policy not set"
code=$(status POST /accounts '{"name":"carl","role":"user","password":"no digits in this one"}')
[[ $code == 422 ]] || fail "a password without a digit: $code $(cat "$work/body")"
code=$(status POST /accounts '{"name":"carl","role":"user","password":"qwertyuiop123"}')
[[ $code == 422 ]] || fail "a common password: $code $(cat "$work/body")"
carl=$(call POST /accounts "$(jq -n --arg p "$carl_secret" '{name:"carl",role:"user",password:$p,password_locked:true}')" \
    | jq -er .id) || fail "carl"
dora=$(call POST /accounts "$(jq -n --arg p "$dora_secret" '{name:"dora",role:"user",password:$p,password_locked:true}')" \
    | jq -er .id) || fail "dora"
echo "   carl $carl, dora $dora; no change of password asked of either"
call PUT /password-policy '{"min_length":10,"mixed_case":false,"digit":false,"symbol":false,"max_age_days":null}' >/dev/null

say "2. A denied group keeps its members out"
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
volume=$(call GET /storage/pools | jq -er --arg pool "$pool" '.[] | select(.name == $pool) | .volume')
guests=$(call POST /groups "{\"name\":\"guests\",\"members\":[$dora]}" | jq -er .id) || fail "guests"
share=$(call POST /shares "$(jq -n --arg pool "$pool" --argjson carl "$carl" --argjson guests "$guests" \
    '{name:"albums",pool:$pool,access:[{kind:"account",id:$carl,access:"read_write"},{kind:"group",id:$guests,access:"read_write"}]}')" \
    | jq -er .id) || fail "albums"
smb_ok albums dora "$dora_secret" 'ls'
jobs=$(call PUT "/shares/access/group/$guests" "{\"rules\":[{\"share_id\":$share,\"rule\":\"deny\"}]}")
for job in $(jq -r '.jobs[].id' <<<"$jobs"); do job_succeeded "$job"; done
smb_refused albums dora "$dora_secret" 'ls'
smb_ok albums carl "$carl_secret" 'ls'
acl_of "/volume$volume/albums" | grep -qx 'user:dora:---' || fail "no entry keeps dora out: $(acl_of "/volume$volume/albums")"
sign_in dora dora "$dora_secret"
call_as dora GET /files/shares | jq -e 'all(.name != "albums")' >/dev/null || fail "dora still sees albums"

echo "   carl joins the denied group, then leaves it"
call PATCH "/groups/$guests" "{\"members\":[$dora,$carl]}" >/dev/null
wait_for "carl's entry" "acl_of /volume$volume/albums | grep -qx 'user:carl:---'"
smb_refused albums carl "$carl_secret" 'ls'
call PATCH "/groups/$guests" "{\"members\":[$dora]}" >/dev/null
wait_for "carl's own rule back" "acl_of /volume$volume/albums | grep -qx 'user:carl:rwx'"
smb_ok albums carl "$carl_secret" 'ls'

say "3. Personal folders"
call PUT /homes "{\"pool\":\"$pool\"}" | jq -e --arg pool "$pool" '.location.pool == $pool' >/dev/null \
    || fail "personal folders not on"
call PUT "/homes/$carl" '{"enabled":true,"quota_bytes":1073741824}' | jq -e '.quota_bytes == 1073741824' >/dev/null \
    || fail "carl got no personal folder"
vm "sudo stat -c '%U %a' /volume$volume/@homes/carl" | grep -qx 'carl 700' || fail "carl's folder is not his alone"
vm "sudo zfs get -H -o value userquota@carl $pool/zendiloos.homes" | grep -qx '1G' || fail "no quota for carl"
vm "echo hello > /tmp/hello.txt"
smb_ok home carl "$carl_secret" 'put /tmp/hello.txt hello.txt; ls'
smb_refused home dora "$dora_secret" 'ls'
sign_in carl carl "$carl_secret"
call_as carl GET /files/shares | jq -e '.[0].home and .[0].id == 0' >/dev/null || fail "carl does not see his folder"
call_as carl GET '/files/shares/0/folder?path=' | jq -e 'any(.entries[]; .name == "hello.txt")' >/dev/null \
    || fail "the file manager does not show what SMB wrote"
call_as dora GET /files/shares | jq -e 'all(.home | not)' >/dev/null || fail "dora sees a personal folder"
code=$(curl -sk -o /dev/null -w '%{http_code}' -b "$work/dora.jar" "$api/files/shares/0/folder?path=")
[[ $code == 404 ]] || fail "dora reached a personal folder: $code"
call PUT /homes '{"pool":null}' >/dev/null
smb_refused home carl "$carl_secret" 'ls'
vm "sudo test -f /volume$volume/@homes/carl/hello.txt" || fail "turning them off lost carl's files"

say "Cleaning up"
job_succeeded "$(call POST "/shares/$share/delete" '{"confirm":"albums"}' | jq -r .id)"
call DELETE "/groups/$guests" >/dev/null
call DELETE "/accounts/$carl" >/dev/null
call DELETE "/accounts/$dora" >/dev/null
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"

say "Passed: password rules, denied groups and their members, personal folders"
