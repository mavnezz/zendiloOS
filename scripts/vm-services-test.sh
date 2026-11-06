#!/usr/bin/env bash
# Tests the file services' settings end to end in the test VM, through the
# HTTPS API, with testparm, systemctl and smbclient inside the VM:
#
#   1. SMB's options reach Samba's configuration as global settings
#   2. a share chosen for Time Machine is offered to Macs as one
#   3. the discovery services follow their switches
#   4. encryption required: a client that cannot encrypt is refused
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Uses two free disks of the VM for a
# pool of its own; puts every setting back as a fresh install has it.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=servicetest
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

job_succeeded() {
    local id=$1 state
    [[ $id =~ ^[0-9]+$ ]] || fail "no job: $id"
    wait_for "job $id" "[[ \$(call GET /jobs/$id | jq -r .state) != running ]]" 300
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "") + " " + (.error_detail // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

# Every parameter, defaults included: -s alone leaves out what equals them.
samba() { vm "testparm -sv 2>/dev/null"; }
has() { grep -qF -- "$1" <<<"$2" || fail "Samba lacks '$1'"; }
lacks() { ! grep -qF -- "$1" <<<"$2" || fail "Samba still has '$1'"; }
state_of() { vm "echo \$(systemctl is-enabled $1 2>/dev/null) \$(systemctl is-active $1 2>/dev/null)"; }

defaults='{"smb":true,"nfs":true,"workgroup":"WORKGROUP","smb_min_protocol":"smb2_1"}'
secret='backups of 5 macbooks'

say "Clients in the VM"
vm "command -v smbclient >/dev/null && command -v avahi-browse >/dev/null \
    || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient avahi-utils >/dev/null"

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"

say "Cleaning up an earlier run"
call PUT /file-services "$defaults" >/dev/null
for id in $(call GET /shares | jq -r '.[] | select(.name == "machines") | .id'); do
    job_succeeded "$(call POST "/shares/$id/delete" '{"confirm":"machines"}' | jq -r .id)"
done
id=$(call GET /accounts | jq -r '.[] | select(.name == "mac") | .id')
[[ -n $id ]] && call DELETE "/accounts/$id" >/dev/null
if call GET /storage/pools | jq -e --arg pool "$pool" '.[] | select(.name == $pool)' >/dev/null; then
    job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"
fi

say "1. Options become Samba's global settings"
text=$(samba)
has 'access based share enum = Yes' "$text"
options='{"smb":true,"nfs":true,"workgroup":"HOME","smb_min_protocol":"smb3","smb_signing":"mandatory",
          "hide_unreadable":false,"oplocks":false,"durable_handles":true,"veto_files":"/*.tmp/Thumbs.db/",
          "delete_veto_files":true,"mac_compat":false}'
call PUT /file-services "$options" | jq -e '.leases == false' >/dev/null || fail "options not stored"
text=$(samba)
for line in 'workgroup = HOME' 'server min protocol = SMB3' 'server signing = required' \
            'durable handles = Yes' 'kernel share modes = No' 'posix locking = No' 'oplocks = No' \
            'smb2 leases = No' 'veto files = /*.tmp/Thumbs.db/' 'delete veto files = Yes'; do
    has "$line" "$text"
done
lacks 'access based share enum = Yes' "$text"

say "2. A Time Machine share"
free=$(call GET /storage/disks | jq -c '[.[] | select((.usage == "unused" or .usage == "foreign") and .id_link != null and (.read_only | not)) | .name][:2]')
[[ $(jq length <<<"$free") == 2 ]] || fail "two free disks are needed, found $free"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"mirror\",\"disks\":$free}")
job_succeeded "$(call POST /storage/pools "{\"token\":\"$(jq -er .token <<<"$plan")\",\"confirm\":\"$pool\"}" | jq -r .id)"
mac=$(call POST /accounts "$(jq -n --arg p "$secret" '{name:"mac",role:"user",password:$p,password_locked:true}')" \
    | jq -er .id) || fail "mac"
share=$(call POST /shares "$(jq -n --arg pool "$pool" --argjson mac "$mac" \
    '{name:"machines",pool:$pool,access:[{kind:"account",id:$mac,access:"read_write"}]}')" | jq -er .id) || fail "machines"
code=$(curl -sk -o /dev/null -w '%{http_code}' -b "$work/admin.jar" -X PUT -H "Origin: $base" \
    -H "X-CSRF-Token: $(cat "$work/admin.csrf")" -H 'Content-Type: application/json' \
    -d "{\"smb\":true,\"nfs\":true,\"workgroup\":\"HOME\",\"smb_min_protocol\":\"smb3\",\"mac_compat\":false,\"time_machine\":{\"share_id\":$share}}" \
    "$api/file-services")
[[ $code == 422 ]] || fail "Time Machine without macOS compatibility: $code"
call PUT /file-services "{\"smb\":true,\"nfs\":true,\"workgroup\":\"WORKGROUP\",\"smb_min_protocol\":\"smb2_1\",\"time_machine\":{\"share_id\":$share,\"max_bytes\":536870912000}}" \
    | jq -e ".time_machine.share_id == $share" >/dev/null || fail "Time Machine not set"
text=$(vm "testparm -sv --section-name=machines 2>/dev/null")
has 'fruit:time machine = yes' "$text"
has 'fruit:time machine max size = 512000M' "$text"
has 'vfs objects = shadow_copy2 catia fruit streams_xattr recycle' "$text"
# smbd announces it as it starts; Avahi can take a while to pass it on.
if ! (wait_for "Bonjour" "vm 'avahi-browse -tpr _adisk._tcp 2>/dev/null' | grep -q 'adVN=machines'" 180); then
    fail "the Time Machine share is not on Bonjour: $(vm 'avahi-browse -tpr _adisk._tcp 2>&1' | head -5)"
fi

say "3. Discovery follows its switches"
call PUT /file-services '{"smb":true,"nfs":true,"workgroup":"WORKGROUP","smb_min_protocol":"smb2_1","wsdd":false,"bonjour":false}' >/dev/null
[[ $(state_of wsdd2) == "disabled inactive" ]] || fail "wsdd2: $(state_of wsdd2)"
[[ $(state_of avahi-daemon) == "disabled inactive" ]] || fail "avahi: $(state_of avahi-daemon)"
has 'multicast dns register = No' "$(samba)"
call PUT /file-services "$defaults" >/dev/null
[[ $(state_of wsdd2) == "enabled active" ]] || fail "wsdd2 back: $(state_of wsdd2)"
[[ $(state_of avahi-daemon) == "enabled active" ]] || fail "avahi back: $(state_of avahi-daemon)"

say "4. Encryption required"
call PUT /file-services '{"smb":true,"nfs":true,"workgroup":"WORKGROUP","smb_min_protocol":"smb2_1","smb_encryption":"required"}' >/dev/null
# A client that cannot encrypt, SMB 2.1, is refused; one with SMB 3 encrypts.
out=$(vm "smbclient //localhost/machines -U 'mac%$secret' -m SMB2_10 -c ls 2>&1" || true)
grep -q NT_STATUS_ACCESS_DENIED <<<"$out" || fail "a client that cannot encrypt connected: $out"
out=$(vm "smbclient //localhost/machines -U 'mac%$secret' -m SMB3 -c ls 2>&1" || true)
! grep -q NT_STATUS <<<"$out" || fail "an SMB 3 client was refused: $out"

say "Cleaning up"
call PUT /file-services "$defaults" >/dev/null
job_succeeded "$(call POST "/shares/$share/delete" '{"confirm":"machines"}' | jq -r .id)"
call DELETE "/accounts/$mac" >/dev/null
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"

say "Passed: SMB options, Time Machine, discovery, encryption"
