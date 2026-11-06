#!/usr/bin/env bash
# Tests the web interface's defences end to end in a test VM, through the
# HTTPS API and nginx:
#
#   1. TLS 1.3 only: a TLS 1.2 client is refused, then accepted again
#   2. an address that fails too often is blocked from everything but the
#      health check; the block is lifted from the VM itself
#   3. an allowed address is never blocked; one's own address cannot be
#   4. an account locks after failed logins and is free again a minute later
#   5. an administrator ends another session
#
# The host's requests reach the VM from QEMU's gateway (10.0.2.2); that is
# the address that gets blocked. Requests from inside the VM come from this
# machine itself, which is never blocked: they lift the block. Everything is
# put back as a fresh install has it. Runs in the second VM by default (the
# first holds a LAN test); account and password as for vm-storage-test.sh.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
port=${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}
base="https://127.0.0.1:$port"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
defaults='{"idle_minutes":30,"account_lock":{"failures":5,"window_minutes":15},
           "auto_block":{"failures":10,"window_minutes":5,"release_after_days":1},"allowed":[]}'
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# Signs `who` in from the host; its cookies and CSRF token go to $work/$who.*.
sign_in() {
    local who=$1 name=$2 secret=$3
    curl -sk -c "$work/$who.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$name" --arg password "$secret" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/$who.csrf"
}

# As `who`, from the host: METHOD PATH [BODY]; prints the body, then the status.
as() {
    local who=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk -b "$work/$who.jar" -c "$work/$who.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/$who.csrf" 2>/dev/null)" -w '\n%{http_code}')
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}
status() { as "$@" | tail -1; }
body() { as "$@" | sed '$d'; }

# A login attempt from the host; prints the status.
attempt() {
    curl -sk -o /dev/null -w '%{http_code}' -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$1" --arg password "$2" '{name: $name, password: $password}')" "$api/auth/login"
}

# As the administrator from inside the VM, which is never blocked; prints
# the status.
inside() {
    local method=$1 path=$2 body=${3:-} login
    login=$(jq -cn --arg name "$user" --arg password "$password" '{name: $name, password: $password}')
    vm 'bash -s' <<EOF
set -e
jar=/tmp/security-test.jar
if [ ! -s /tmp/security-test.csrf ]; then
    curl -sk -c \$jar -H 'Origin: https://127.0.0.1' -H 'Content-Type: application/json' -d '$login' \
        https://127.0.0.1/api/v1/auth/login | grep -o '"csrf_token":"[^"]*"' | cut -d'"' -f4 >/tmp/security-test.csrf
fi
curl -sk -b \$jar -o /dev/null -w '%{http_code}' -X $method -H 'Origin: https://127.0.0.1' \
    -H "X-CSRF-Token: \$(cat /tmp/security-test.csrf)" -H 'Content-Type: application/json' \
    ${body:+-d '$body'} https://127.0.0.1/api/v1$path
EOF
}

handshake() {
    openssl s_client -connect "127.0.0.1:$port" "$1" -brief </dev/null >/dev/null 2>&1
}

# nginx reloads in the background: new workers take over a moment later.
eventually() {
    for _ in $(seq 1 20); do
        if "$@"; then return 0; fi
        sleep 0.5
    done
    return 1
}

restore() {
    set +e
    inside PUT /security/settings "$(jq -c . <<<"$defaults")" >/dev/null
    inside PUT /system/tls '{"minimum":"tls1_2"}' >/dev/null
    inside POST /security/unblock '{"address":"10.0.2.2"}' >/dev/null
    vm 'rm -f /tmp/security-test.jar /tmp/security-test.csrf' >/dev/null 2>&1
    rm -rf "$work"
}
trap restore EXIT

say "Signing in as $user"
sign_in admin "$user" "$password" || fail "cannot sign in"
[[ $(status admin PUT /security/settings "$defaults") == 200 ]] || fail "defaults"

say "1. TLS 1.3 only"
[[ $(status admin PUT /system/tls '{"minimum":"tls1_2"}') == 200 ]] || fail "TLS 1.2 and newer"
vm 'grep -qx "ssl_protocols TLSv1.2 TLSv1.3;" /etc/zendiloos/nginx/tls.conf' || fail "no TLS settings file"
eventually handshake -tls1_2 || fail "TLS 1.2 refused"
[[ $(body admin PUT /system/tls '{"minimum":"tls1_3"}') == '{"minimum":"tls1_3"}' ]] || fail "not set"
eventually eval '! handshake -tls1_2' || fail "a TLS 1.2 client still gets in"
handshake -tls1_3 || fail "TLS 1.3 refused"
[[ $(status admin PUT /system/tls '{"minimum":"tls1_2"}') == 200 ]] || fail "back to 1.2"
eventually handshake -tls1_2 || fail "TLS 1.2 still refused"

say "2. Blocked after failed logins, the health check stays open"
[[ $(status admin PUT /security/settings \
    '{"idle_minutes":30,"account_lock":null,"auto_block":{"failures":3,"window_minutes":5,"release_after_days":1}}') == 200 ]] \
    || fail "settings"
for _ in 1 2 3; do attempt nobody wrong >/dev/null; done
code=$(attempt "$user" "$password")
[[ $code == 403 ]] || fail "a blocked address signed in: $code"
[[ $(status admin GET /system/info) == 403 ]] || fail "the admin's session from that address still works"
[[ $(curl -sk -o /dev/null -w '%{http_code}' "$api/health") == 200 ]] || fail "health check"
[[ $(inside GET /security/blocked) == 200 ]] || fail "the VM itself is refused"
[[ $(inside POST /security/unblock '{"address":"10.0.2.2"}') == 204 ]] || fail "cannot lift the block"
[[ $(status admin GET /system/info) == 200 ]] || fail "still blocked after lifting"
body admin GET '/logs/audit?action=identity.address_block&limit=1' \
    | jq -e '.entries[0].target == "10.0.2.2"' >/dev/null || fail "no audit entry"

say "3. Allowed addresses stay, one's own cannot be blocked"
[[ $(status admin PUT /security/settings \
    '{"idle_minutes":30,"account_lock":null,"auto_block":{"failures":3,"window_minutes":5,"release_after_days":1},"allowed":["10.0.2.0/24"]}') == 200 ]] \
    || fail "settings"
for _ in 1 2 3 4; do attempt nobody wrong >/dev/null; done
[[ $(status admin GET /system/info) == 200 ]] || fail "an allowed address was blocked"
[[ $(status admin POST /security/blocked '{"address":"10.0.2.0/16"}') == 409 ]] || fail "own address blocked"

say "4. Accounts lock, and are free again"
# nginx lets ten logins through at once, then one every six seconds.
sleep 30
[[ $(status admin POST /accounts \
    '{"name":"locktest","role":"user","password":"a long and lonely pass","password_locked":true}') == 201 ]] \
    || fail "account"
[[ $(status admin PUT /security/settings \
    '{"idle_minutes":30,"account_lock":{"failures":3,"window_minutes":1},"auto_block":null}') == 200 ]] || fail "settings"
for _ in 1 2 3; do attempt locktest wrong >/dev/null; done
[[ $(attempt locktest "a long and lonely pass") == 429 ]] || fail "not locked"
sleep 61
[[ $(attempt locktest "a long and lonely pass") == 200 ]] || fail "still locked"

say "5. An administrator ends another session"
sign_in other locktest "a long and lonely pass" || fail "second session"
id=$(body admin GET /security/sessions | jq -er '[.[] | select(.account == "locktest")][0].id') || fail "not listed"
[[ $(status admin DELETE "/security/sessions/$id") == 204 ]] || fail "not ended"
[[ $(status other GET /system/info) == 401 ]] || fail "the ended session still works"
id=$(body admin GET /accounts | jq -r '.[] | select(.name == "locktest") | .id')
[[ $(status admin DELETE "/accounts/$id") == 204 ]] || fail "cleanup"

say "All security tests passed"
