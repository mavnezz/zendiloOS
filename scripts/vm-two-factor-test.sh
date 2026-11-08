#!/usr/bin/env bash
# Tests two-factor sign-in end to end in a test VM, through the HTTPS API,
# with the codes computed here (RFC 6238) and smbclient inside the VM:
#
#   1. setting it up asks for the password; the code from the app confirms it
#      and yields the recovery codes
#   2. the password alone then gives no session, only a challenge; the code
#      from the app completes it, the same code a second time does not
#   3. a recovery code works once
#   4. three wrong codes end the pending sign-in
#   5. the second step from another address is refused, the sign-in stays
#      open where the password came from
#   6. SMB keeps working with the password alone
#   7. turning it off asks for the password and a code; an administrator's
#      reset turns it off as well, and the password alone signs in again
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Creates the account `tfatest` and
# deletes it again. Waits for the next 30-second step where a fresh code is
# needed, so it takes about three minutes.
set -euo pipefail

# The second VM unless told otherwise: the first holds a LAN test.
export ZENDILOOS_VM=${ZENDILOOS_VM:-2}

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
name=tfatest
secret="Tfa-$(head -c 12 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')-9"
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

# nginx lets 10 password and code checks a minute through, then answers 503:
# runs curl (printing the status code) until it gets through.
paced() {
    local got
    for _ in $(seq 1 20); do
        got=$(curl "$@")
        [[ $got == 503 ]] || { printf '%s' "$got"; return; }
        sleep 6
    done
    printf '%s' "$got"
}

# method path [body] as a session: the status code, the body in $work/body.
status_as() {
    local session=$1 method=$2 path=$3 body=${4:-}
    local args=(-sk -o "$work/body" -w '%{http_code}' -b "$work/$session.jar" -c "$work/$session.jar"
                -X "$method" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/$session.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    paced "${args[@]}" "$api$path"
}
expect() { # wanted-status session method path [body]
    local wanted=$1; shift
    local got
    got=$(status_as "$@")
    [[ $got == "$wanted" ]] || fail "$2 $3: $got instead of $wanted: $(cat "$work/body")"
}

# The password step into a fresh cookie jar: 200 or 202, the body in $work/body.
login() { # session password
    rm -f "$work/$1.jar" "$work/$1.csrf"
    paced -sk -o "$work/body" -w '%{http_code}' -c "$work/$1.jar" -H "Origin: $base" \
        -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$name" --arg password "$2" '{name: $name, password: $password}')" \
        "$api/auth/login"
}
# The second step for the challenge in $work/token.
second() { # session code
    paced -sk -o "$work/body" -w '%{http_code}' -b "$work/$1.jar" -c "$work/$1.jar" -H "Origin: $base" \
        -H 'Content-Type: application/json' \
        -d "$(jq -n --arg token "$(cat "$work/token")" --arg code "$2" '{token: $token, code: $code}')" \
        "$api/auth/login/second-factor"
}
has_session_cookie() { grep -q zendiloos "$work/$1.jar" 2>/dev/null; }

# The code of the authenticator app for the secret, now.
totp() {
    python3 -I - "$1" <<'EOF'
import base64, hashlib, hmac, struct, sys, time
key = sys.argv[1].upper()
key = base64.b32decode(key + '=' * (-len(key) % 8))
digest = hmac.new(key, struct.pack('>Q', int(time.time()) // 30), hashlib.sha1).digest()
offset = digest[-1] & 15
print('%06d' % ((struct.unpack('>I', digest[offset:offset + 4])[0] & 0x7FFFFFFF) % 1_000_000))
EOF
}
# A code is good once: wait for the next step (and away from its edge).
next_step() { sleep $((31 - $(date +%s) % 30)); }

cleanup() {
    set +e
    if [[ -s $work/admin.csrf ]]; then
        [[ -s $work/settings ]] && status_as admin PUT /security/settings "$(cat "$work/settings")" >/dev/null
        id=$(status_as admin GET /accounts >/dev/null; jq -r --arg n "$name" '.[] | select(.name == $n) | .id' "$work/body")
        [[ -n $id ]] && status_as admin DELETE "/accounts/$id" >/dev/null
    fi
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf" || fail "cannot sign in as $user"
skew=$(( $(vm "date +%s") - $(date +%s) ))
(( skew > -10 && skew < 10 )) || fail "the VM's clock is ${skew} seconds off"

# The test fails on purpose more often than the default limits allow: the
# account would lock and this machine's address be blocked. Wider limits
# while it runs, the old ones back afterwards.
expect 200 admin GET /security/settings
cp "$work/body" "$work/settings"
expect 200 admin PUT /security/settings "$(jq '.account_lock = {failures: 100, window_minutes: 1}
    | .auto_block = (.auto_block // {}) + {failures: 100, window_minutes: 1}' "$work/settings")"

status_as admin GET /accounts >/dev/null
old=$(jq -r --arg n "$name" '.[] | select(.name == $n) | .id' "$work/body")
[[ -n $old ]] && expect 204 admin DELETE "/accounts/$old"
initial="Init-$(head -c 12 /dev/urandom | base64 | tr -dc 'A-Za-z0-9')-1"
expect 201 admin POST /accounts "$(jq -n --arg n "$name" --arg p "$initial" '{name: $n, role: "user", password: $p}')"
account=$(jq -r .id "$work/body")
# A new account chooses its own password first.
[[ $(login user "$initial") == 200 ]] || fail "no session with the first password: $(cat "$work/body")"
jq -er .csrf_token "$work/body" >"$work/user.csrf"
expect 204 user POST /auth/password "$(jq -n --arg old "$initial" --arg new "$secret" \
    '{current_password: $old, new_password: $new}')"

say "1. Setting it up: the password first, then a code"
[[ $(login user "$secret") == 200 ]] || fail "no session with the password: $(cat "$work/body")"
jq -er .csrf_token "$work/body" >"$work/user.csrf"
expect 200 user GET /auth/two-factor
[[ $(jq -r .enabled "$work/body") == false ]] || fail "two-factor is on from the start"
expect 422 user POST /auth/two-factor/setup '{"password":"not the password"}'
[[ $(jq -r .code "$work/body") == wrong_password ]] || fail "a wrong password: $(cat "$work/body")"
expect 200 user POST /auth/two-factor/setup "$(jq -n --arg p "$secret" '{password: $p}')"
key=$(jq -r .secret "$work/body")
jq -r .uri "$work/body" | grep -q '^otpauth://totp/' || fail "no otpauth URI: $(cat "$work/body")"
expect 422 user POST /auth/two-factor/confirm '{"code":"000000"}'
expect 200 user POST /auth/two-factor/confirm "$(jq -n --arg c "$(totp "$key")" '{code: $c}')"
mapfile -t recovery < <(jq -r '.recovery_codes[]' "$work/body")
(( ${#recovery[@]} == 10 )) || fail "${#recovery[@]} recovery codes instead of 10"
expect 200 user GET /auth/two-factor
[[ $(jq -r .enabled "$work/body") == true ]] || fail "two-factor is not on after confirming"
expect 200 admin GET /accounts
[[ $(jq -r --arg n "$name" '.[] | select(.name == $n) | .two_factor' "$work/body") == true ]] \
    || fail "the account list does not show two-factor"

say "2. The password alone gives a challenge, the code a session, once"
next_step
[[ $(login second "$secret") == 202 ]] || fail "the password alone: $(cat "$work/body")"
jq -r .token "$work/body" >"$work/token"
has_session_cookie second && fail "a session cookie after the password step"
code=$(totp "$key")
[[ $(second second "$code") == 200 ]] || fail "the app's code: $(cat "$work/body")"
jq -er .csrf_token "$work/body" >"$work/second.csrf"
expect 200 second GET /auth/session
[[ $(login third "$secret") == 202 ]] || fail "the password alone, again: $(cat "$work/body")"
jq -r .token "$work/body" >"$work/token"
[[ $(second third "$code") == 422 ]] || fail "the same code twice: $(cat "$work/body")"

say "3. A recovery code works once"
[[ $(second third "${recovery[0]}") == 200 ]] || fail "a recovery code: $(cat "$work/body")"
[[ $(login fourth "$secret") == 202 ]] || fail "the password alone: $(cat "$work/body")"
jq -r .token "$work/body" >"$work/token"
[[ $(second fourth "${recovery[0]}") == 422 ]] || fail "a recovery code twice: $(cat "$work/body")"

say "4. Three wrong codes end the sign-in"
[[ $(second fourth 111111) == 422 ]] || fail "a wrong code: $(cat "$work/body")"
second fourth 222222 >/dev/null
second fourth 333333 >/dev/null
[[ $(second fourth "${recovery[1]}") == 401 ]] || fail "a good code after three wrong ones: $(cat "$work/body")"
[[ $(jq -r .code "$work/body") == sign_in_expired ]] || fail "after three wrong codes: $(cat "$work/body")"
# A good sign-in clears the failures before the next steps.
[[ $(login fifth "$secret") == 202 ]] || fail "the password alone: $(cat "$work/body")"
jq -r .token "$work/body" >"$work/token"
[[ $(second fifth "${recovery[2]}") == 200 ]] || fail "a recovery code: $(cat "$work/body")"

say "5. The second step from another address is refused"
[[ $(login sixth "$secret") == 202 ]] || fail "the password alone: $(cat "$work/body")"
token=$(jq -r .token "$work/body")
# Through nginx inside the VM: the client is 127.0.0.1 there, not the host.
got=$(vm "curl -sk -o /dev/null -w '%{http_code}' -H 'Origin: https://localhost' \
    -H 'Content-Type: application/json' -d '{\"token\":\"$token\",\"code\":\"${recovery[3]}\"}' \
    https://localhost/api/v1/auth/login/second-factor")
[[ $got == 401 ]] || fail "the second step from inside the VM: $got"
printf '%s\n' "$token" >"$work/token"
# Refused there, it stays open where the password came from.
[[ $(second sixth "${recovery[3]}") == 200 ]] || fail "the sign-in from the right address: $(cat "$work/body")"

say "6. SMB with the password alone"
vm "command -v smbclient >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq smbclient >/dev/null"
vm "smbclient -L //localhost -U '$name%$secret' >/dev/null 2>&1" || fail "SMB refuses the password"

say "7. Turning it off, and an administrator's reset"
next_step
expect 422 user POST /auth/two-factor/disable "$(jq -n --arg c "$(totp "$key")" '{password: "not the password", code: $c}')"
next_step
expect 204 user POST /auth/two-factor/disable "$(jq -n --arg p "$secret" --arg c "$(totp "$key")" '{password: $p, code: $c}')"
[[ $(login seventh "$secret") == 200 ]] || fail "the password alone after turning it off: $(cat "$work/body")"
jq -er .csrf_token "$work/body" >"$work/seventh.csrf"
expect 200 seventh POST /auth/two-factor/setup "$(jq -n --arg p "$secret" '{password: $p}')"
key=$(jq -r .secret "$work/body")
next_step
expect 200 seventh POST /auth/two-factor/confirm "$(jq -n --arg c "$(totp "$key")" '{code: $c}')"
[[ $(login eighth "$secret") == 202 ]] || fail "the password alone with two-factor on again"
expect 204 admin DELETE "/accounts/$account/two-factor"
[[ $(login ninth "$secret") == 200 ]] || fail "the password alone after the reset: $(cat "$work/body")"

say "Two-factor sign-in works."
