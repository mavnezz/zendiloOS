#!/usr/bin/env bash
# Tests renaming the machine end to end in a test VM, through the HTTPS
# API, with hostname, testparm, avahi-resolve and openssl inside the VM:
#
#   1. names the API refuses (a dot, too long, digits only) change nothing
#   2. renamed: the host name (static and the kernel's), /etc/hosts,
#      Samba's NetBIOS name, <name>.local through Avahi, the web
#      interface's own certificate (served by nginx at once), the API
#   3. systemd was not reloaded for it, and no unit is behind the disk
#   4. the name put back, also when a step failed
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Runs in the second VM by default (the
# first holds a LAN test). The VM is not restarted: cloud-init, which set
# the VM's name, keeps a name changed since.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
new=${ZENDILOOS_TEST_NAME:-zdlrenamed}
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

status_of() {
    curl -sk -o /dev/null -w '%{http_code}' -b "$work/admin.jar" -X PUT -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/admin.csrf")" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$1" '{name: $name}')" "$api/system/hostname"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-30}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The NetBIOS name Samba takes now, as a fresh process sees the host name.
netbios() { vm "testparm -s --parameter-name='netbios name' 2>/dev/null"; }
# The names the certificate nginx serves gives.
served_names() {
    vm "echo | openssl s_client -connect 127.0.0.1:443 2>/dev/null \
        | openssl x509 -noout -ext subjectAltName 2>/dev/null | tail -n1"
}
# From the journal's last entry on: a time could be older than entries a
# clock set wrong once wrote. systemd names who asked for each reload.
journal_end() { vm 'sudo journalctl -q -n 0 --show-cursor' | sed -n 's/^-- cursor: //p'; }
reloads_since() {
    vm "sudo journalctl -q --after-cursor '$1' _PID=1 -g 'Reload requested from client' -o cat" \
        | grep -c 'unit zendiloos-agent.service' || true
}
restarted_since() {
    local lines
    lines=$(vm "sudo journalctl -q --after-cursor '$2' -u '$1' -o cat")
    grep -q '^Started' <<<"$lines"
}

original=
restore() {
    set +e
    if [[ -n $original && -s $work/admin.csrf ]]; then
        say "Putting the name $original back"
        call PUT /system/hostname "$(jq -n --arg name "$original" '{name: $name}')" >/dev/null
        [[ $(vm hostname) == "$original" ]] || printf 'The VM is still called %s\n' "$(vm hostname)" >&2
    fi
    rm -rf "$work"
}
trap restore EXIT

say "Clients in the VM"
vm "command -v avahi-resolve >/dev/null && command -v testparm >/dev/null \
    || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq avahi-utils samba-common-bin >/dev/null"

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
original=$(call GET /system/hostname | jq -er .name) || fail "no name: $(call GET /system/hostname)"
[[ $original == "$(vm hostname)" ]] || fail "the API says $original, the VM $(vm hostname)"
[[ $original != "$new" ]] || fail "the VM is called $new already: set ZENDILOOS_TEST_NAME"
say "The VM is called $original"

say "1. Names the API refuses change nothing"
for bad in nas.local a-very-long-name 1234 -nas localhost; do
    [[ $(status_of "$bad") == 422 ]] || fail "$bad was not refused"
done
[[ $(vm hostname) == "$original" ]] || fail "renamed by a refused name"

say "2. Renamed to $new"
cursor=$(journal_end)
[[ -n $cursor ]] || fail "no cursor of the journal"
renamed=$(call PUT /system/hostname "$(jq -n --arg name "$new" '{name: $name}')")
jq -e --arg name "$new" '.name == $name and .netbios_name == ($name | ascii_upcase)' <<<"$renamed" >/dev/null \
    || fail "not renamed: $renamed"
[[ $(vm hostname) == "$new" ]] || fail "the kernel's name is $(vm hostname)"
[[ $(vm "cat /etc/hostname") == "$new" ]] || fail "/etc/hostname: $(vm 'cat /etc/hostname')"
vm "grep -qw '$new' /etc/hosts && ! grep -qw '$original' /etc/hosts" \
    || fail "/etc/hosts: $(vm 'cat /etc/hosts')"
vm "getent hosts '$new' >/dev/null" || fail "$new does not resolve on the VM"
[[ $(netbios) == "${new^^}" ]] || fail "Samba's NetBIOS name is $(netbios)"
if vm "systemctl is-active --quiet avahi-daemon"; then
    wait_for "$new.local through Avahi" "vm \"avahi-resolve -4 -n '$new.local'\" | grep -q '$new.local'"
    old_answer=$(vm "avahi-resolve -4 -n '$original.local' 2>/dev/null" || true)
    [[ -z $old_answer ]] || fail "Avahi still answers for $original.local: $old_answer"
    # Read whole first: grep -q stops at the first match, and avahi-browse
    # writing on would end with a broken pipe, which pipefail counts.
    browsed=$(vm "avahi-browse -tpr _smb._tcp 2>/dev/null")
    grep -q "${new^^}" <<<"$browsed" || fail "Samba's Bonjour service is not called ${new^^}: $browsed"
else
    say "   (Avahi does not run: Bonjour left out)"
fi
for unit in smbd wsdd2; do
    if vm "systemctl is-active --quiet $unit"; then
        restarted_since "$unit" "$cursor" || fail "$unit was not restarted for the new name"
    fi
done
if jq -e '.own_certificate_served' <<<"$renamed" >/dev/null; then
    jq -e '.certificate_renewed' <<<"$renamed" >/dev/null || fail "the certificate was not made anew"
    served_names | grep -q "DNS:$new, DNS:$new.local" || fail "nginx serves: $(served_names)"
fi
vm "sudo openssl x509 -noout -subject -in /etc/zendiloos/tls/zendiloos.crt" | grep -q "CN *= *$new" \
    || fail "the own certificate names $(vm 'sudo openssl x509 -noout -subject -in /etc/zendiloos/tls/zendiloos.crt')"
[[ $(vm "sudo stat -c %a /etc/zendiloos/tls/zendiloos.key") == 600 ]] || fail "the key is not private"
call GET /system/info | jq -e --arg name "$new" '.hostname == $name' >/dev/null \
    || fail "the information says $(call GET /system/info | jq .hostname)"
call GET /logs/audit | jq -e '[.. | strings | select(test("system.host_name_update"))] | length > 0' \
    >/dev/null || fail "not in the audit log"

say "3. systemd was not reloaded for it"
reloads=$(reloads_since "$cursor")
[[ $reloads == 0 ]] || fail "the agent reloaded systemd $reloads times for a rename"
for unit in smbd avahi-daemon nginx; do
    [[ $(vm "systemctl show -p NeedDaemonReload --value $unit") != yes ]] || fail "$unit is behind the disk"
done

say "4. The name put back"
call PUT /system/hostname "$(jq -n --arg name "$original" '{name: $name}')" | jq -e .name >/dev/null \
    || fail "cannot put $original back"
[[ $(vm hostname) == "$original" ]] || fail "still called $(vm hostname)"
[[ $(netbios) == "${original^^}" ]] || fail "Samba's NetBIOS name is $(netbios)"
original=

say "All checks passed"
