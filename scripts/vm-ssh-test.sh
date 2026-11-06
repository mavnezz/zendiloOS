#!/usr/bin/env bash
# Tests SSH as a service end to end in a test VM, through the HTTPS API,
# with sshd -T, ss and ssh-keyscan inside the VM:
#
#   1. a new port and rules reach sshd; the old port closes
#   2. a port another program listens on is refused
#   3. turned off, SSH takes no new connections, open sessions stay
#   4. turned on from off, with a timer that turns it off again by itself
#   5. back on port 22 with the system's rules, as before the test
#
# While the port moves or SSH is off, new connections fail: the test runs
# its commands over one connection opened at the start, which stays, and
# puts SSH back on port 22 through the API whatever happens. Runs in the
# second VM by default (the first holds a LAN test); account and password
# as for vm-storage-test.sh.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
sock=$work/ssh.sock

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
# Over the connection opened at the start.
vm() { "$root/scripts/vm.sh" ssh -o ControlMaster=no -o ControlPath="$sock" "$@"; }
# A new connection, as a client would open one.
fresh() { "$root/scripts/vm.sh" ssh -o ControlMaster=no -o ControlPath=none true 2>/dev/null; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/admin.jar" -c "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

status_of() {
    local body=$1
    curl -sk -o /dev/null -w '%{http_code}' -b "$work/admin.jar" -X PUT -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/admin.csrf")" -H 'Content-Type: application/json' -d "$body" \
        "$api/system/ssh"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-30}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The rules sshd works with, after every drop-in.
effective() { vm "sudo sshd -T 2>/dev/null | grep -E '^(port|permitrootlogin|passwordauthentication) ' | sort"; }
listening() { [[ -n $(vm "ss -Hltn 'sport = :$1'") ]]; }

restore() {
    set +e
    if [[ -s $work/admin.csrf ]]; then
        call PUT /system/ssh '{"enabled":true,"port":22}' >/dev/null
    fi
    vm 'sudo systemctl stop ssh-test-listener 2>/dev/null' >/dev/null 2>&1
    "$root/scripts/vm.sh" ssh -o ControlPath="$sock" -O exit 2>/dev/null
    rm -rf "$work"
}
trap restore EXIT

say "Opening the connection that stays"
"$root/scripts/vm.sh" ssh -o ControlMaster=yes -o ControlPath="$sock" -o ControlPersist=yes -fN \
    || fail "no SSH to the VM"
before=$(effective)

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in"
call GET /system/ssh | jq -e '.installed and .running' >/dev/null || fail "SSH: $(call GET /system/ssh)"

say "1. A new port and rules reach sshd"
call PUT /system/ssh '{"enabled":true,"port":2222,"root_login":"no","password_login":false}' \
    | jq -e '.running and .port == 2222 and .root_login == "no" and .password_login == false' >/dev/null \
    || fail "not applied: $(call GET /system/ssh)"
rules=$(effective)
for line in 'port 2222' 'permitrootlogin no' 'passwordauthentication no'; do
    grep -qx "$line" <<<"$rules" || fail "sshd lacks '$line': $rules"
done
wait_for "the listener on 2222" "listening 2222"
wait_for "port 22 to close" "! listening 22"
vm 'ssh-keyscan -p 2222 127.0.0.1 2>/dev/null' | grep -q ssh-ed25519 || fail "sshd does not answer on 2222"
! fresh || fail "a new connection to port 22 still works"

say "2. A port in use is refused"
vm 'sudo systemd-run --quiet --unit=ssh-test-listener python3 -m http.server 8080'
wait_for "the listener on 8080" "listening 8080"
code=$(status_of '{"enabled":true,"port":8080}')
[[ $code == 409 ]] || fail "port in use: $code"
code=$(status_of '{"enabled":true,"port":445}')
[[ $code == 422 ]] || fail "port 445: $code"
vm 'sudo systemctl stop ssh-test-listener'
call GET /system/ssh | jq -e '.port == 2222' >/dev/null || fail "the port changed anyway"

say "3. Off: no new connections, open sessions stay"
call PUT /system/ssh '{"enabled":false,"port":2222}' | jq -e '(.running or .enabled) | not' >/dev/null \
    || fail "still on: $(call GET /system/ssh)"
[[ $(vm 'systemctl is-enabled ssh; systemctl is-active ssh' | paste -sd' ') == 'disabled inactive' ]] \
    || fail "ssh.service: $(vm 'systemctl is-enabled ssh; systemctl is-active ssh' | paste -sd' ')"
wait_for "the listener to close" "! listening 2222"
vm true || fail "the open session ended"

say "4. On from off, and off again by itself"
call PUT /system/ssh '{"enabled":true,"port":2222,"off_after_minutes":1}' \
    | jq -e '.running and .off_at != null' >/dev/null || fail "not on: $(call GET /system/ssh)"
vm 'systemctl list-timers --all zendiloos-ssh-off.timer' | grep -q zendiloos-ssh-off || fail "no timer"
wait_for "the timer to turn SSH off" "call GET /system/ssh | jq -e '(.running or .enabled or .off_at) | not'" 100
vm true || fail "the open session ended"

say "5. Back on port 22 with the system's rules"
call PUT /system/ssh '{"enabled":true,"port":22}' | jq -e '.running and .port == 22 and .off_at == null' >/dev/null \
    || fail "not back: $(call GET /system/ssh)"
[[ $(effective) == "$before" ]] || fail "rules differ from before: $(effective) / $before"
wait_for "a new connection" fresh
vm 'cat /etc/ssh/sshd_config.d/00-zendiloos.conf' | grep -qx 'Port 22' || fail "settings file"

say "All SSH tests passed"
