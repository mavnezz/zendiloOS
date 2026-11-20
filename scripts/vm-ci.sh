#!/usr/bin/env bash
# Runs a group of the VM tests (docs/testing.md) in fresh test VMs, as the
# nightly workflow .github/workflows/vm-tests.yml does on GitHub:
#
#   scripts/vm-ci.sh list      the groups, with their tests in order
#   scripts/vm-ci.sh groups    the groups' names only, one per line
#   scripts/vm-ci.sh <group>   creates the group's VMs, starts them, installs
#                              the package, sets zendiloOS up, makes what the
#                              group's tests need (pools, Docker, an app), then
#                              runs the tests in order, stopping at the first
#                              that fails
#
# The VMs are made afresh for the run: ZENDILOOS_VM (2 by default, the
# tests' own default) and, for the groups that need a second one or a
# throwaway one, ZENDILOOS_CI_SECOND_VM (the next number). 'create' refuses a
# VM that exists, so nothing kept in one is touched; never on VM 1, which
# holds a LAN test. Passed, the VMs are destroyed (ZENDILOOS_CI_KEEP=1 keeps
# them); after a failure they keep running, for a look with scripts/vm.sh
# ssh, and are destroyed by hand (ZENDILOOS_VM=<n> scripts/vm.sh destroy).
#
# dev/vm-ci/<group>/ holds each test's output, summary.md with the times
# and, after a failure, each VM's console and the journals of zendiloosd
# and zendiloos-agent from inside it.
#
# The packages:
#   ZENDILOOS_DEB          the one to test; the newest dist/zendiloos_*_amd64.deb
#   ZENDILOOS_CI_NEXT_DEB  for vm-update-test, one of a newer version (as
#                          ZENDILOOS_VERSION=<version>.vmtest1 just deb builds
#                          it); the newest dist/next/zendiloos_*_amd64.deb
#   ZENDILOOS_CI_OLD_DEB   for vm-upgrade-test, an older one; by default the
#                          latest release's from GitHub, fetched into dev/vm-ci/
#
# Each VM has 2 vCPUs and 2 GB, as scripts/vm.sh makes them; a group with a
# second VM needs 4 GB and the disks of both (thin, a few GB each).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
vmsh=$root/scripts/vm.sh
main_vm=${ZENDILOOS_VM:-2}
second_vm=${ZENDILOOS_CI_SECOND_VM:-$((main_vm + 1))}
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
repository=mavnezz/zendiloOS

# The groups, each run in VMs of its own, and their tests in the order they
# run. The order matters where a test leaves the VM as the next one needs it
# or would be disturbed by the one before (see each group's preparation).
groups=(storage sharing security docker vms network restore pool-import upgrade)
declare -A tests=(
    [storage]="storage pool-delete enclosure removable notify"
    [sharing]="sharing permissions services accounts backup"
    [security]="security two-factor ssh hostname web-server ups"
    [docker]="docker-install docker app-center project-pins docker-volumes firewall"
    [vms]="network vms vm-backup"
    [network]="network-takeover network-bridge"
    [restore]="config-backup"
    [pool-import]="pool-import"
    [upgrade]="upgrade update"
)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
usage() { sed -n '2,/^set /{/^set /d;p}' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

case ${1:-} in
    list)
        for group in "${groups[@]}"; do printf '%-12s %s\n' "$group" "${tests[$group]}"; done
        exit 0 ;;
    groups) printf '%s\n' "${groups[@]}"; exit 0 ;;
    '' | -h | --help) usage ;;
esac
group=$1
[[ -n ${tests[$group]:-} ]] || { echo "no group $group; scripts/vm-ci.sh list names them" >&2; exit 1; }
for n in "$main_vm" "$second_vm"; do
    [[ $n =~ ^[2-9]$ ]] || fail "the VMs must be 2 to 9, not $n: VM 1 holds a LAN test"
done
[[ $main_vm != "$second_vm" ]] || fail "ZENDILOOS_VM and ZENDILOOS_CI_SECOND_VM are both $main_vm"

deb=${ZENDILOOS_DEB:-$(ls -t "$root"/dist/zendiloos_*_amd64.deb 2>/dev/null | head -1)}
[[ -n $deb && -f $deb ]] || fail "no package; run 'just deb' or set ZENDILOOS_DEB"
deb=$(realpath "$deb")

out=$root/dev/vm-ci/$group
rm -rf "$out"
mkdir -p "$out"
work=$(mktemp -d)
summary=$out/summary.md
# The VMs this run made: destroyed once it passed, looked into if it failed.
made=()
started=$SECONDS

base() { echo "https://127.0.0.1:$((8442 + $1))"; }
on() { local vm=$1; shift; ZENDILOOS_VM=$vm "$vmsh" ssh "$@"; }
dir_of() { if [[ $1 == 1 ]]; then echo "$root/dev/vm"; else echo "$root/dev/vm$1"; fi; }
minutes() { printf '%d:%02d' $(($1 / 60)) $(($1 % 60)); }

wait_for() {
    local what=$1 check=$2 seconds=${3:-180}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# call VM METHOD PATH [BODY]: as the administrator signed in on that VM.
call() {
    local vm=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk --max-time 60 -b "$work/$vm.jar" -c "$work/$vm.jar" -X "$method" -H "Origin: $(base "$vm")"
                -H "X-CSRF-Token: $(cat "$work/$vm.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$(base "$vm")/api/v1$path"
}

sign_in() {
    local vm=$1
    curl -sk -c "$work/$vm.jar" -H "Origin: $(base "$vm")" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$(base "$vm")/api/v1/auth/login" | jq -er .csrf_token >"$work/$vm.csrf"
}

job_succeeded() {
    local vm=$1 id=$2 seconds=${3:-600} state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job on VM $vm: $id"
    wait_for "job $id on VM $vm" "[[ \$(call $vm GET /jobs/$id | jq -r .state) != running ]]" "$seconds"
    state=$(call "$vm" GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id on VM $vm: $state"
}

note_time() { printf '| %s | %s | %s |\n' "$1" "$2" "$(minutes "$3")" >>"$summary"; }

# A VM made afresh and started; with "installed", the package goes in too.
new_vm() {
    local vm=$1 what=${2:-}
    say "VM $vm: created and started${what:+, the package installed}"
    ZENDILOOS_VM=$vm "$vmsh" create
    made+=("$vm")
    ZENDILOOS_VM=$vm "$vmsh" start
    [[ $what != installed ]] || ZENDILOOS_VM=$vm "$vmsh" install "$deb"
}

# The first setup, as the browser does it, with the setup code from the VM.
set_up() {
    local vm=$1 code
    wait_for "zendiloosd on VM $vm" "curl -sfk $(base "$vm")/api/v1/health" 180
    code=$(on "$vm" 'sudo cat /var/lib/zendiloos/setup-code')
    curl -sfk -o /dev/null -H "Origin: $(base "$vm")" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg code "$code" --arg name "$user" --arg password "$password" \
            '{setup_code: $code, name: $name, password: $password}')" \
        "$(base "$vm")/api/v1/setup" || fail "the first setup on VM $vm"
    sign_in "$vm" || fail "cannot sign in on VM $vm"
    say "VM $vm: set up, $user signed in"
}

# A pool of one disk, NVMe or SATA (by the serial numbers scripts/vm.sh
# gives them), so that the SATA disks the tests take for their own pools
# stay free: four of them, and with tank on one, three.
make_pool() {
    local vm=$1 name=$2 kind=$3 disk plan token
    disk=$(call "$vm" GET /storage/disks | jq -r --arg kind "ZDL-$kind-" '[.[] | select(.usage == "unused"
        and .id_link != null and (.read_only | not) and ((.serial // "") | startswith($kind)))][0].name // empty')
    [[ -n $disk ]] || fail "no free $kind disk on VM $vm for the pool $name"
    plan=$(call "$vm" POST /storage/pools/plan "$(jq -nc --arg n "$name" --arg d "$disk" '{name: $n, layout: "single", disks: [$d]}')")
    token=$(jq -er .token <<<"$plan") || fail "the plan of $name: $plan"
    job_succeeded "$vm" "$(call "$vm" POST /storage/pools "$(jq -nc --arg t "$token" --arg n "$name" '{token: $t, confirm: $n}')" | jq -r .id)"
    say "VM $vm: the pool $name on $disk"
}

make_share() {
    local vm=$1 name=$2 pool=$3 me
    me=$(call "$vm" GET /auth/session | jq -er .account.id) || fail "no session on VM $vm"
    call "$vm" POST /shares "$(jq -nc --arg n "$name" --arg p "$pool" --argjson me "$me" \
        '{name: $n, pool: $p, access: [{kind: "account", id: $me, access: "read_write"}]}')" | jq -er .id \
        || fail "no share $name on VM $vm"
}

# Docker installed as the App Center installs it, its data on the pool.
docker_on() {
    local vm=$1 pool=$2
    job_succeeded "$vm" "$(call "$vm" POST /apps/docker/install "$(jq -nc --arg p "$pool" '{pool: $p}')" | jq -r .id)" 1800
    call "$vm" GET /apps/docker | jq -e --arg p "$pool" '.running and .volume.pool == $p' >/dev/null \
        || fail "Docker on VM $vm: $(call "$vm" GET /apps/docker)"
    say "VM $vm: Docker $(call "$vm" GET /apps/docker | jq -r .version), its data on $pool"
}

# An app of the official catalog deployed as an administrator's project into
# a folder of the share, as the App Center does: vm-docker-test follows the
# container uptime-kuma-uptime-kuma-1 while Docker's data moves, as the test
# VM it was written on had one.
app_project() {
    local vm=$1 app=$2 share=$3 port=$4 source compose reply id
    call "$vm" GET /apps/catalog | jq -e --arg a "$app" 'any(.[]; .id == $a and .official)' >/dev/null \
        || call "$vm" POST /apps/sources/refresh >/dev/null
    wait_for "$app in the catalog on VM $vm" \
        "call $vm GET /apps/catalog | jq -e --arg a $app 'any(.[]; .id == \$a and .official)'" 120
    source=$(call "$vm" GET /apps/catalog | jq -er --arg a "$app" '[.[] | select(.id == $a and .official)][0].source_id')
    compose=$(call "$vm" POST "/apps/catalog/$source/$app/compose" "$(jq -nc --arg a "$app" --argjson s "$share" \
        --argjson p "$port" '{name: $a, share_id: $s, folder: ("docker/" + $a), ports: {web: $p}}')" | jq -er .compose) \
        || fail "cannot fill $app's template on VM $vm"
    reply=$(call "$vm" POST /docker/projects "$(jq -nc --arg a "$app" --argjson s "$share" --arg c "$compose" \
        --argjson src "$source" '{name: $a, share_id: $s, folder: ("docker/" + $a), compose: $c, start: true,
            template: {source_id: $src, app: $a}}')")
    id=$(jq -er .job.id <<<"$reply") || fail "the project $app on VM $vm: $reply"
    job_succeeded "$vm" "$id" 900
    wait_for "$app to run on VM $vm" \
        "[[ \$(on $vm \"sudo docker inspect -f '{{.State.Running}}' $app-$app-1\") == true ]]" 300
    say "VM $vm: $app runs as a project in the share's folder docker/$app"
}

nfs_on() {
    local vm=$1 services
    services=$(call "$vm" GET /file-services)
    if ! jq -e .nfs <<<"$services" >/dev/null; then
        call "$vm" PUT /file-services "$(jq -c '.nfs = true' <<<"$services")" | jq -e .nfs >/dev/null \
            || fail "cannot turn NFS on on VM $vm"
    fi
    say "VM $vm: NFS on"
}

# The latest release's package, for the upgrade test: through gh when it is
# there (signed in, or GH_TOKEN in CI), else through GitHub's API.
old_package() {
    local dir=$root/dev/vm-ci/old url
    if [[ -n ${ZENDILOOS_CI_OLD_DEB:-} ]]; then realpath "$ZENDILOOS_CI_OLD_DEB"; return; fi
    rm -rf "$dir" && mkdir -p "$dir"
    if command -v gh >/dev/null && gh release download --repo "$repository" --pattern 'zendiloos_*_amd64.deb' \
        --dir "$dir" >&2; then
        :
    else
        url=$(curl -fsSL "https://api.github.com/repos/$repository/releases/latest" \
            | jq -er '.assets[] | select(.name | test("^zendiloos_.*_amd64\\.deb$")) | .browser_download_url') \
            || fail "no package in the latest release"
        curl -fsSL -o "$dir/${url##*/}" "$url"
    fi
    ls "$dir"/zendiloos_*_amd64.deb
}

# Each group's VMs and what its tests need beyond a VM set up (their
# headers in scripts/vm-*-test.sh). The VM's number is in ZENDILOOS_VM for
# every test unless run_test is told otherwise.
prepare() {
    case $group in
        storage | security)
            new_vm "$main_vm" installed
            set_up "$main_vm" ;;
        sharing)
            # vm-backup-test backs a share on apps up into tank; Docker is
            # for the step of vm-permissions-test that runs a container in
            # zendiloos-files (skipped without it); the other tests make
            # pools of their own on two of the three SATA disks left.
            new_vm "$main_vm" installed
            set_up "$main_vm"
            make_pool "$main_vm" apps NVME
            make_pool "$main_vm" tank SATA
            docker_on "$main_vm" apps
            nfs_on "$main_vm" ;;
        docker)
            new_vm "$main_vm" installed
            set_up "$main_vm"
            make_pool "$main_vm" apps NVME
            docker_on "$main_vm" apps
            share=$(make_share "$main_vm" appdata apps)
            app_project "$main_vm" uptime-kuma "$share" 3001 ;;
        vms)
            # vm-vms-test puts the VMs' storage on apps and its second
            # location on tank.
            new_vm "$main_vm" installed
            set_up "$main_vm"
            make_pool "$main_vm" apps NVME
            make_pool "$main_vm" tank SATA ;;
        network)
            # Throwaway, with the two spare cards vm-network-bridge-test
            # bonds; vm-network-takeover-test leaves it with ifupdown, which
            # the bridge test takes over again first.
            export ZENDILOOS_VM_EXTRA_NICS=2
            new_vm "$main_vm" installed
            set_up "$main_vm" ;;
        restore)
            # The configuration backup of the main VM, with a share for its
            # scheduled backups, restored on the second instead of its
            # first setup.
            new_vm "$main_vm" installed
            new_vm "$second_vm" installed
            set_up "$main_vm"
            make_pool "$main_vm" tank SATA
            make_share "$main_vm" files tank >/dev/null ;;
        pool-import)
            # Installed, never set up: the test sets it up itself, then
            # reinstalls it. Throwaway (3 or more), so the second VM's number.
            new_vm "$second_vm" installed ;;
        upgrade)
            # Nothing installed: the test installs the older package, then
            # this one; the update test follows on the VM it leaves.
            old_deb=$(old_package)
            say "The older package: $(basename "$old_deb")"
            next_deb=${ZENDILOOS_CI_NEXT_DEB:-$(ls -t "$root"/dist/next/zendiloos_*_amd64.deb 2>/dev/null | head -1)}
            [[ -n $next_deb && -f $next_deb ]] \
                || fail "no newer package for the update test: ZENDILOOS_VERSION=<version>.vmtest1 just deb, into dist/next/"
            next_deb=$(realpath "$next_deb")
            new_vm "$main_vm" ;;
    esac
}

run_test() {
    local name=$1 began=$SECONDS env=(ZENDILOOS_VM="$main_vm") args=()
    case $name in
        config-backup) env+=(ZENDILOOS_SOURCE_VM="$main_vm" ZENDILOOS_TARGET_VM="$second_vm") ;;
        pool-import) env=(ZENDILOOS_VM="$second_vm" ZENDILOOS_DEB="$deb") ;;
        upgrade) args=("$old_deb" "$deb") ;;
        update) args=("$next_deb" "$deb") ;;
    esac
    say "== vm-$name-test (${env[*]}${args[*]:+ ${args[*]##*/}})"
    if env "${env[@]}" "$root/scripts/vm-$name-test.sh" "${args[@]}" </dev/null 2>&1 | tee "$out/$name.log"; then
        note_time "vm-$name-test" passed $((SECONDS - began))
    else
        note_time "vm-$name-test" "**failed**" $((SECONDS - began))
        fail "vm-$name-test; its output is in ${out#"$root"/}/$name.log"
    fi
    # nginx lets ten sign-ins a minute through from an address
    # (packaging/nginx/zendiloos.conf); these tests use them up on purpose,
    # and the next test's first sign-in would be refused.
    case $name in
        security | two-factor) say "A minute's rest for nginx's limit of sign-ins"; sleep 60 ;;
    esac
}

# What there is to see of each VM after a failure: its console, and the
# journals from inside it, started again for that should a test have left
# it powered off (its console of the boot before kept first).
look_into() {
    local vm
    for vm in "${made[@]}"; do
        cp "$(dir_of "$vm")/console.log" "$out/vm$vm-console.log" 2>/dev/null || true
        if ! timeout 30 env ZENDILOOS_VM="$vm" "$vmsh" ssh true 2>/dev/null; then
            timeout 300 env ZENDILOOS_VM="$vm" "$vmsh" start >/dev/null 2>&1 || true
        fi
        for unit in zendiloosd zendiloos-agent; do
            timeout 120 env ZENDILOOS_VM="$vm" "$vmsh" ssh \
                "sudo journalctl -u $unit --no-pager -o short-iso-precise" >"$out/vm$vm-$unit.log" 2>&1 || true
        done
    done
    echo "The VMs keep running: ${made[*]/#/VM }. Afterwards: ZENDILOOS_VM=<n> scripts/vm.sh destroy" >&2
}

finish() {
    local status=$1 vm
    rm -rf "$work"
    printf '\n%s in %s\n' "$([[ $status == 0 ]] && echo Passed || echo Failed)" "$(minutes $((SECONDS - started)))" >>"$summary"
    if [[ $status != 0 ]]; then
        (( ${#made[@]} == 0 )) || look_into
    elif [[ ${ZENDILOOS_CI_KEEP:-} != 1 ]]; then
        for vm in "${made[@]}"; do ZENDILOOS_VM=$vm "$vmsh" destroy >/dev/null; done
    fi
}
trap 'finish $?' EXIT

printf '### VM tests: %s\n\n%s\n\n| Step | Result | Time |\n|---|---|---|\n' \
    "$group" "$(basename "$deb")" >"$summary"
began=$SECONDS
prepare
note_time "VMs made and prepared" done $((SECONDS - began))
for name in ${tests[$group]}; do run_test "$name"; done
say "Passed: ${tests[$group]} (group $group, $(minutes $((SECONDS - started))))"
