#!/usr/bin/env bash
# Installs Docker the way the App Store does, end to end in a test VM:
# Docker's Debian packages are removed (their configuration stays), then
# installed again through the API as a job, by apt in a unit of its own
# outside the agent's sandbox, and its data put back on the pool. Apps on
# it run again afterwards.
#
# Needs the VM running with the package installed and set up, Docker's
# data on a pool and the network for apt; account and password as for
# vm-storage-test.sh. Runs in the second VM by default (the first holds a
# LAN test): it takes Docker away for a while.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

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

wait_for() {
    local what=$1 check=$2 seconds=${3:-120}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
pool=$(call GET /apps/docker | jq -er .volume.pool) || fail "Docker's data is not on a pool"
apps=$(vm 'sudo docker ps -q | wc -l')

say "1. Docker's packages go; their configuration and the data stay"
vm 'sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y remove docker.io docker-compose docker-cli' >/dev/null
call GET /apps/docker | jq -e '.installed == false' >/dev/null || fail "still installed: $(call GET /apps/docker)"

say "2. Installed again through the store, as a job"
job=$(call POST /apps/docker/install "{\"pool\":\"$pool\"}" | jq -er .id) || fail "install refused"
wait_for "the job" "[[ \$(call GET /jobs/$job | jq -r .state) != running ]]" 1800
state=$(call GET "/jobs/$job" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
[[ $state == succeeded* ]] || fail "job $job: $state"
record=$(vm 'cat /var/lib/zendiloos-packages/run.json') || fail "the run was not recorded"
jq -e '.state == "succeeded" and .task == "install_docker"' <<<"$record" >/dev/null || fail "the record: $record"
setup=$(call GET /apps/docker)
jq -e --arg p "$pool" '.installed and .running and .volume.pool == $p and (.version | length > 0)' <<<"$setup" >/dev/null \
    || fail "Docker after the install: $setup"
[[ $(call POST /apps/docker/install "{\"pool\":\"$pool\"}" | jq -r .code) == docker_installed ]] \
    || fail "a second install was not refused"

say "3. The apps on it run again"
wait_for "$apps containers" "(( \$(vm 'sudo docker ps -q | wc -l') >= $apps ))" 180

say "All Docker install tests passed (Docker $(jq -r .version <<<"$setup"))"
