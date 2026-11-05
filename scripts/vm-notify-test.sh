#!/usr/bin/env bash
# Tests notification channels in the test VM, through the HTTPS API:
#
#   1. a webhook to a small receiver inside the VM gets a test message
#   2. an email channel to a mail server that is not there reports why
#   3. a real notification (a finished scrub of a one-disk pool) reaches
#      the webhook on its own
#
# Needs the VM running with the package installed and set up, and one free
# SATA disk. Account and password come from ZENDILOOS_USER and
# ZENDILOOS_PASSWORD (defaults: admin, "correct horse battery").
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ${ZENDILOOS_VM:-1}))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
pool=vmnotify
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/jar" -c "$work/jar" -X "$method" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

# The status code of a request, its body in $work/body.
status() {
    local method=$1 path=$2
    curl -sk -o "$work/body" -w '%{http_code}' -b "$work/jar" -X "$method" -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/csrf")" "$api$path"
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
    state=$(call GET "/jobs/$id" | jq -r '.state + " " + (.error_code // "")')
    [[ $state == succeeded* ]] || fail "job $id: $state"
}

channels=()
cleanup() {
    for id in "${channels[@]}"; do call DELETE "/notification-channels/$id" >/dev/null || true; done
    if call GET /storage/pools | jq -e --arg pool "$pool" '.pools[] | select(.name == $pool)' >/dev/null 2>&1; then
        call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" >/dev/null || true
    fi
    vm 'pkill -f zendiloos-hook-receiver || true' >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user"
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in"

say "1. A webhook receives a test message"
# Logs every request body, one per line, and answers 204.
vm "rm -f /tmp/hooks.log; nohup python3 -c '
import http.server, sys
class Hook(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers[\"Content-Length\"]))
        open(\"/tmp/hooks.log\", \"ab\").write(body + b\"\\n\")
        self.send_response(204); self.end_headers()
    def log_message(self, *args): pass
sys.argv[0] = \"zendiloos-hook-receiver\"
http.server.HTTPServer((\"127.0.0.1\", 8099), Hook).serve_forever()
' zendiloos-hook-receiver >/dev/null 2>&1 &"
wait_for "the receiver" "vm 'ss -Hltn | grep -q 127.0.0.1:8099'" 20
hook=$(call POST /notification-channels '{"name":"Test hook","target":{"kind":"webhook","url":"http://127.0.0.1:8099/hook","format":"json"},"min_level":"info","language":"en"}')
hook_id=$(jq -er .id <<<"$hook") || fail "webhook: $hook"
channels+=("$hook_id")
[[ $(status POST "/notification-channels/$hook_id/test") == 204 ]] || fail "the test did not go out: $(cat "$work/body")"
vm 'grep -q "Test message" /tmp/hooks.log' || fail "the receiver got nothing"
vm 'tail -n1 /tmp/hooks.log' | jq -e '.title | startswith("[") and contains("Test message")' >/dev/null \
    || fail "the message does not name the NAS: $(vm 'tail -n1 /tmp/hooks.log')"

say "2. An email channel without a mail server says why"
mail=$(call POST /notification-channels '{"name":"Test mail","target":{"kind":"email","server":"127.0.0.1","port":2525,"security":"none","from":"nas@example.org","to":["admin@example.org"]},"min_level":"error","language":"de"}')
mail_id=$(jq -er .id <<<"$mail") || fail "email channel: $mail"
channels+=("$mail_id")
[[ $(status POST "/notification-channels/$mail_id/test") == 502 ]] || fail "a test to nowhere went out: $(cat "$work/body")"
jq -e '.code == "delivery_failed" and (.detail | length > 0)' "$work/body" >/dev/null || fail "no reason: $(cat "$work/body")"
echo "   $(jq -r .detail "$work/body")"
call GET /notification-channels | jq -e --argjson id "$mail_id" '.[] | select(.id == $id) | .last_error != null' >/dev/null \
    || fail "the failure is not kept"

say "3. A real notification reaches the webhook"
disk=$(call GET /storage/disks | jq -r '[.[] | select((.usage == "unused" or .usage == "foreign")
    and (.name | startswith("sd")) and .id_link != null and (.read_only | not)) | .name][0] // empty')
[[ -n $disk ]] || fail "a free SATA disk is needed"
plan=$(call POST /storage/pools/plan "{\"name\":\"$pool\",\"layout\":\"single\",\"disks\":[\"$disk\"]}")
token=$(jq -er .token <<<"$plan") || fail "plan: $plan"
job_succeeded "$(call POST /storage/pools "{\"token\":\"$token\",\"confirm\":\"$pool\"}" | jq -r .id)"
[[ $(status POST "/storage/pools/$pool/scrub") == 204 ]] || fail "cannot scrub: $(cat "$work/body")"
# The monitor looks once a minute.
wait_for "the scrub's notification at the webhook" "vm 'grep -q \"Data check of $pool\" /tmp/hooks.log'" 150
vm "grep \"Data check of $pool\" /tmp/hooks.log | tail -n1" | jq -e '.code == "scrub_finished" and .level == "info"' >/dev/null \
    || fail "the notification: $(vm 'tail -n1 /tmp/hooks.log')"
job_succeeded "$(call POST "/storage/pools/$pool/destroy" "{\"confirm\":\"$pool\"}" | jq -r .id)"

say "Passed: webhook, email failure reported, a scrub's notification delivered"
