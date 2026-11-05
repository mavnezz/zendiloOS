#!/usr/bin/env bash
# Tests pool management end to end in the test VM, through the HTTPS API as
# the interface uses it:
#
#   1. create a ZFS mirror on two free disks (plan, confirm, job)
#   2. write data to its volume
#   3. pull one disk out of the kernel, so that the pool degrades
#   4. replace it with a third disk through the API and wait for the resilver
#   5. verify the data, run a scrub, then destroy the pool
#
# Along the way it waits for the monitor's notifications about the degraded
# and the recovered pool (the monitor looks once a minute).
#
# Needs the VM running with the package installed and set up. Account and
# password come from ZENDILOOS_USER and ZENDILOOS_PASSWORD (defaults: admin,
# "correct horse battery"). Uses three free SATA disks of the VM (a SATA disk
# can be pulled out of the kernel); pools in use stay alone, and the removed
# disk is brought back at the end.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-8443}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=vmtest
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# curl with the session; -k: the VM's certificate is self-signed.
call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

job_succeeded() {
    local id=$1 state
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]"
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

pool_json() { call GET /storage/pools | jq -c --arg pool "$pool" '.[] | select(.name == $pool)'; }

say "Signing in as $user"
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in"

since=$(call GET '/notifications?limit=1' | jq '.[0].id // 0')
notified() {
    call GET '/notifications?limit=50' \
        | jq -e --arg pool "$pool" --arg code "$1" --argjson since "$since" \
            '.[] | select(.id > $since and .code == $code and .params.pool == $pool)'
}

if [[ -n $(pool_json) ]]; then
    say "Removing pool $pool left from an earlier run"
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi

free=$(call GET /storage/disks | jq -r '[.[] | select((.usage == "unused" or .usage == "foreign")
    and (.name | startswith("sd")) and .id_link != null and (.read_only | not)) | .name][:3] | join(" ")')
read -r first second spare <<<"$free"
[[ -n ${spare:-} ]] || fail "three free SATA disks are needed, found: ${free:-none}"

say "1. Planning a mirror on $first and $second"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":[\"$first\",\"$second\"]}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
mount=$(jq -r .mount_point <<<"$plan")
echo "   erases: $(jq -r '[.disks[] | .id_link] | join(", ")' <<<"$plan"); mounts at $mount"
[[ $(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"wrong\"}" | jq -r .code) == confirmation_mismatch ]] \
    || fail "a wrong confirmation was accepted"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
[[ $(pool_json | jq -r .state) == online ]] || fail "pool not online: $(pool_json)"

say "2. Writing data to $mount"
vm "findmnt -n $mount >/dev/null && sudo dd if=/dev/urandom of=$mount/data.bin bs=1M count=300 status=none && sync && sha256sum $mount/data.bin" \
    | tee "$work/checksum"

say "3. Pulling $second out of the kernel"
vm "echo 1 | sudo tee /sys/block/$second/device/delete >/dev/null; sudo dd if=/dev/urandom of=$mount/more.bin bs=1M count=50 status=none; sync"
wait_for "the pool to degrade" "[[ \$(pool_json | jq -r .state) == degraded ]]" 120
missing=$(pool_json | jq -r '.devices[] | select(.state != "online") | .guid')
echo "   missing device: $missing"
wait_for "a notification about the degraded pool" "notified pool_trouble" 150
echo "   notified: $(notified pool_trouble | jq -r '.params.state')"

say "4. Replacing it with $spare"
replace=$(call POST "/storage/pools/$pool/replace/plan" "{\"device\":\"$missing\",\"disk\":\"$spare\"}")
token=$(jq -er .token <<<"$replace") || fail "replace plan: $replace"
job_succeeded "$(call POST "/storage/pools/$pool/replace" "{\"token\":\"$token\"}" | jq -r .id)"
wait_for "the resilver" "[[ \$(pool_json | jq -r '.state + \" \" + (.scan.kind // \"\") + \" \" + (.scan.state // \"\")') == 'online resilver finished' ]]" 600
echo "   $(pool_json | jq -r '.devices | map(.disk + "=" + .state) | join(", ")')"
wait_for "a notification about the recovered pool" "notified pool_recovered" 150
echo "   notified: pool_recovered"

say "5. Verifying the data, checking it, destroying the pool"
vm "sha256sum -c --quiet" <"$work/checksum" || fail "data changed"
call POST "/storage/pools/$pool/scrub" >/dev/null
wait_for "the scrub" "[[ \$(pool_json | jq -r '(.scan.kind // \"\") + \" \" + (.scan.state // \"\")') == 'scrub finished' ]]" 300
errors=$(pool_json | jq -r .scan.errors)
[[ $errors == 0 ]] || fail "the scrub found $errors errors"
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
[[ -z $(pool_json) ]] || fail "the pool still exists"
vm "! findmnt -n $mount" || fail "$mount is still mounted"

say "Bringing $second back"
vm 'for host in /sys/class/scsi_host/host*/scan; do echo "- - -" | sudo tee $host >/dev/null; done; sudo udevadm settle'
say "Passed: create, degrade, notify, replace, resilver, notify, scrub and destroy work"
