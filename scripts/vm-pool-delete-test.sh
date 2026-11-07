#!/usr/bin/env bash
# Tests deleting pools, end to end in a test VM, through the HTTPS API:
#
#   1. a pool of its own is made on a free disk, with a share on it: the
#      delete is refused and names the share
#   2. without the share, but with its mount busy (a process keeps a folder
#      there open): the delete fails and leaves the pool as it was,
#      mounted; once nothing holds it, it is deleted
#
# Only the pool it makes is ever deleted, so a build without these checks
# loses nothing else. Needs the VM running with the package installed and
# set up and a disk nothing uses (it is erased). Runs in the second VM by
# default (the first holds a LAN test).
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=scratch
share=scratchshare
holder=/run/zendiloos-test-busy.pid
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
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 600
    call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")'
}

# Ends the process holding the mount, if there is one.
release() { vm "sudo sh -c 'test -s $holder && kill \$(cat $holder); rm -f $holder'" 2>/dev/null || true; }

exists() { call GET /storage/pools | jq -e --arg p "$pool" 'any(.[]; .name == $p)' >/dev/null; }

destroy() { status POST "/storage/pools/$1/destroy" "{\"confirm\":\"$1\"}"; }

remove_share() {
    for id in $(call GET /shares | jq -r --arg s "$share" '.[] | select(.name == $s) | .id'); do
        id=$(call POST "/shares/$id/delete" "{\"confirm\":\"$share\"}" | jq -r .id)
        [[ $id =~ ^[0-9]+$ ]] && job_end "$id" >/dev/null
    done
}

cleanup() {
    set +e
    release
    remove_share
    if exists; then
        id=$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)
        [[ $id =~ ^[0-9]+$ ]] && job_end "$id" >/dev/null
    fi
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
sign_in || fail "cannot sign in"

say "1. A pool that holds a share is not deleted"
remove_share
if ! exists; then
    disk=$(call GET /storage/disks | jq -r '.[] | select(.usage == "unused" and .id_link != null) | .name' | head -1)
    [[ -n $disk ]] || fail "no free disk"
    plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"single\",\"disks\":[\"$disk\"]}")
    id=$(call POST /storage/pools "{\"token\":\"$(jq -er .token <<<"$plan")\",\"confirm\":\"$pool\"}" | jq -er .id) \
        || fail "no pool: $plan"
    state=$(job_end "$id")
    [[ $state == succeeded* ]] || fail "creating $pool: $state"
fi
me=$(call GET /auth/session | jq -er .account.id)
call POST /shares "$(jq -n --arg n "$share" --arg pool "$pool" --argjson me "$me" \
    '{name:$n,pool:$pool,access:[{kind:"account",id:$me,access:"read_write"}]}')" | jq -e .id >/dev/null || fail "share"
[[ $(destroy "$pool") == 409 ]] || fail "the pool with a share: $(cat "$work/last")"
jq -e --arg s "share $share" '.code == "pool_in_use" and (.detail | contains($s))' "$work/last" >/dev/null \
    || fail "the refusal: $(cat "$work/last")"
echo "  refused: $(jq -r .detail "$work/last")"
remove_share

say "2. A busy mount leaves a pool as it was"
mount=$(call GET /storage/pools | jq -r --arg p "$pool" '.[] | select(.name == $p) | .mount_point')
# A process outside any unit: systemd would stop a unit that needs the
# mount before unmounting it, and so make it free.
vm "sudo setsid bash -c 'cd $mount && echo \$\$ > $holder && exec sleep 600' </dev/null >/dev/null 2>&1 &"
wait_for "the process holding $mount" "vm 'sudo test -s $holder'" 30
[[ $(destroy "$pool") == 202 ]] || fail "destroy: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == failed* ]] || fail "a busy pool went: $state"
echo "  refused: ${state#failed }"
exists || fail "the pool went"
vm "findmnt -n $mount" >/dev/null || fail "$mount is not mounted any more"
release
[[ $(destroy "$pool") == 202 ]] || fail "destroy: $(cat "$work/last")"
state=$(job_end "$(jq -r .id "$work/last")")
[[ $state == succeeded* ]] || fail "deleting $pool: $state"
! exists || fail "the pool is still there"
vm "! findmnt -n $mount" >/dev/null || fail "$mount is still mounted"

say "All pool delete tests passed"
