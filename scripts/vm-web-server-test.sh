#!/usr/bin/env bash
# Tests the web interface's settings and its certificates end to end in a
# test VM, through the HTTPS API from inside the VM (the host's forwarded
# port reaches only 443, which step 1 moves away from), with nginx and a
# stand-in for acme.sh:
#
#   1. a new HTTPS port, confirmed, is saved. Had zendiloosd stopped before
#      the agent heard of the confirmation, the host would have gone back to
#      the settings before: with https.conf put back by hand (and nginx
#      reloaded), a restart of zendiloosd brings the file and nginx back to
#      the settings saved
#   2. two confirmations of one change sent at once: one is taken (204), the
#      other finds nothing waiting (409), and the settings saved agree with
#      nginx's files
#   3. issuing with a stand-in for acme.sh, which hands over a chain made
#      here: a good chain is put in place; one with a block that is no
#      certificate, and one whose second certificate did not issue the
#      first, are refused, and the pair served stays as it was
#   4. the web interface shows that certificate. With nginx's reload made to
#      fail (a drop-in, ExecReload=/bin/false), issuing it again fails as not
#      taken: the certificate shows why, nginx still serves the one before,
#      and a notification tells. Without the drop-in, the daily look takes
#      it: nginx serves the new one, the error is gone, and a notification
#      says it was renewed
#
# Nothing starts the daily look but zendiloosd's start, five minutes before
# the first one: step 4 restarts zendiloosd and waits for it, so the test
# takes about eight minutes. Not covered here: a pair that openssl takes but
# nginx refuses once it is in place (the agent's unit tests have that one).
# Needs the VM running with the package installed and set up, and openssl
# in it; account and password as for vm-storage-test.sh. Everything is put
# back as it was: the web interface's settings as a fresh install has them
# (HTTP on 80 to HTTPS on 443, the machine's own certificate), acme.sh as it
# was (or none), no drop-in; the test's certificate, challenge and ACME
# account go. Runs in the second VM by default (the first holds a LAN test).
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
work=$(mktemp -d)
default='{"http_port":80,"https_port":443,"certificate_id":null}'
domain=nas.example.test
# In the VM: the administrator's cookies and CSRF token, where acme.sh is
# looked for, what the stand-in hands over, and the drop-in for nginx.
jar=/tmp/web-server-test.jar
csrf=/tmp/web-server-test.csrf
acme=/usr/share/acme.sh
made=$acme/zendiloos-test
dropin=/etc/systemd/system/nginx.service.d/zendiloos-web-server-test.conf
served=/etc/zendiloos/web/certificates

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

wait_for() {
    local what=$1 check=$2 seconds=${3:-60}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The web interface at PORT inside the VM; also the Origin a browser sends
# there, which leaves out 443.
url() { if [[ $1 == 443 ]]; then echo https://127.0.0.1; else echo "https://127.0.0.1:$1"; fi; }

# Whether nginx serves the web interface at PORT inside the VM.
serves() { [[ $(vm "curl -sk -m 5 -o /dev/null -w '%{http_code}' $(url "$1")/api/v1/health") == 200 ]]; }

# Signs the administrator in from inside the VM, at PORT.
sign_in() {
    vm "curl -sk -m 30 -c $jar -H 'Origin: $(url "$1")' -H 'Content-Type: application/json' --data-binary @- \
        $(url "$1")/api/v1/auth/login | grep -o '\"csrf_token\":\"[^\"]*\"' | cut -d'\"' -f4 >$csrf && test -s $csrf" \
        <<<"$(jq -cn --arg name "$user" --arg password "$password" '{name: $name, password: $password}')"
}

# As the administrator from inside the VM, at PORT: METHOD PATH [BODY];
# prints the body, then the status.
inside() {
    local at=$1 method=$2 path=$3 body=${4:-}
    vm "curl -sk -m 60 -b $jar -c $jar -X $method -H 'Origin: $(url "$at")' \
        -H \"X-CSRF-Token: \$(cat $csrf)\" ${body:+-H 'Content-Type: application/json' --data-binary @-} \
        -w '\n%{http_code}' $(url "$at")/api/v1$path" <<<"$body"
}
status() { inside "$@" | tail -1; }
body() { inside "$@" | sed '$d'; }

# Applies web interface settings at port FROM and confirms them at port TO,
# once nginx serves it there, as the browser does.
change() {
    local from=$1 to=$2 settings=$3 reply
    reply=$(inside "$from" PUT /system/web-server "$settings")
    [[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying $settings: $reply"
    wait_for "the web interface at port $to" "serves $to" 30
    [[ $(status "$to" POST /system/web-server/confirm) == 204 ]] || fail "confirming $settings"
}

# Whether the settings saved, seen at PORT, are SETTINGS with nothing waiting.
saved() {
    body "$1" GET /system/web-server | jq -e --argjson web "$2" '.current == $web and .pending == null'
}

conf() { vm "sudo cat /etc/zendiloos/web/$1"; }
no_previous() { vm "! sudo ls /etc/zendiloos/web/ | grep -q previous"; }

# The chain the stand-in hands over next: the files of $made named, in order.
chain() { vm "cd $made && sudo sh -c 'cat $* >fullchain.cer'"; }

# Issues the test's certificate and waits for the job; prints the job.
issue() {
    local reply job
    reply=$(inside 443 POST "/certificates/$certificate/issue")
    [[ $(tail -1 <<<"$reply") == 202 ]] || fail "issuing: $reply"
    job=$(sed '$d' <<<"$reply" | jq -r .id)
    wait_for "the issuance" "body 443 GET /jobs/$job | jq -e '.finished_at != null'" 120
    body 443 GET "/jobs/$job"
}

# The serial number of the certificate nginx serves, and of the one in place.
serial_served() {
    vm "echo | openssl s_client -connect 127.0.0.1:443 -servername $domain 2>/dev/null | openssl x509 -noout -serial"
}
serial_in_place() { vm "sudo openssl x509 -noout -serial -in $served/$certificate/fullchain.pem"; }
pair_in_place() { vm "sudo cat $served/$certificate/fullchain.pem $served/$certificate/key.pem | sha256sum"; }

# A notification of CODE about the test's certificate since SECONDS (the
# VM's clock).
told() {
    body 443 GET /notifications | jq -e --arg code "$1" --arg name "$domain" --argjson since "$2" \
        '.notifications[] | select(.code == $code and .params.name == $name and .at >= $since)'
}

# acme.sh's place taken by a stand-in that hands over $made/fullchain.cer and
# $made/key.pem as acme.sh would write them, and what it hands over made:
# a CA, a certificate it issued twice (serial numbers 2 and 3) for one key,
# another CA, and a block that is no certificate. The systemd unit acme.sh
# runs in reads /usr but not /tmp or /home.
stand_in() {
    vm "sudo bash -s" <<EOF
set -e
mkdir -p $acme
if [ -e $acme/acme.sh ] && [ ! -e $acme/acme.sh.zendiloos-test ]; then
    mv $acme/acme.sh $acme/acme.sh.zendiloos-test
fi
cat >$acme/acme.sh <<'STUB'
#!/bin/sh
# A stand-in for acme.sh, from zendiloOS's vm-web-server-test.sh.
while [ \$# -gt 0 ]; do
    case \$1 in
        --cert-home) home=\$2; shift ;;
        --domain) [ -n "\$main" ] || main=\$2; shift ;;
    esac
    shift
done
echo "Issuing \$main (a stand-in)"
mkdir -p "\$home/\${main}_ecc"
cp $made/fullchain.cer "\$home/\${main}_ecc/fullchain.cer"
cp $made/key.pem "\$home/\${main}_ecc/\$main.key"
echo "Cert success."
STUB
chmod 0755 $acme/acme.sh
rm -rf $made
mkdir -p $made
cd $made
cat >openssl.cnf <<'CNF'
[req]
distinguished_name = dn
[dn]
[ca]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign
subjectKeyIdentifier = hash
[leaf]
basicConstraints = critical,CA:FALSE
authorityKeyIdentifier = keyid
subjectAltName = DNS:$domain
CNF
ec="-newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -config openssl.cnf"
openssl req -x509 \$ec -extensions ca -days 90 -subj '/CN=zendiloOS test CA' -keyout ca.key -out ca.pem 2>/dev/null
openssl req -x509 \$ec -extensions ca -days 90 -subj '/CN=Another CA' -keyout other.key -out other.pem 2>/dev/null
openssl req -new \$ec -subj '/CN=$domain' -keyout key.pem -out leaf.csr 2>/dev/null
for serial in 2 3; do
    openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -set_serial \$serial -days 60 \
        -extfile openssl.cnf -extensions leaf -out leaf-\$serial.pem 2>/dev/null
done
printf -- '-----BEGIN CERTIFICATE-----\nTUlJQgo=\n-----END CERTIFICATE-----\n' >junk.pem
chmod 0644 ./*
EOF
}

stand_in_gone() {
    vm "sudo bash -s" <<EOF
rm -rf $made
if [ -e $acme/acme.sh.zendiloos-test ]; then
    mv $acme/acme.sh.zendiloos-test $acme/acme.sh
else
    rm -f $acme/acme.sh
    rmdir $acme 2>/dev/null || true
fi
EOF
}

cleanup() {
    set +e
    vm "sudo rm -f $dropin && sudo systemctl daemon-reload" >/dev/null 2>&1
    local at=443
    serves 443 || at=8443
    if sign_in "$at" >/dev/null 2>&1; then
        if [[ ${changed:-no} == yes ]] && ! saved "$at" "$default" >/dev/null 2>&1; then
            # A change still waiting goes back by itself first.
            for _ in $(seq 1 24); do
                [[ $(status "$at" PUT /system/web-server "$default") == 202 ]] && break
                sleep 5
            done
            for _ in $(seq 1 30); do serves 443 && break; sleep 1; done
            status 443 POST /system/web-server/confirm >/dev/null
        fi
        [[ -n ${certificate:-} ]] && status 443 DELETE "/certificates/$certificate" >/dev/null
        [[ -n ${challenge:-} ]] && status 443 DELETE "/certificates/challenges/$challenge" >/dev/null
        [[ -n ${acme_account:-} ]] && status 443 DELETE "/certificates/accounts/$acme_account" >/dev/null
    fi
    [[ ${stand_in:-no} == yes ]] && stand_in_gone >/dev/null 2>&1
    vm "rm -f $jar $csrf" >/dev/null 2>&1
    rm -rf "$work"
}
trap cleanup EXIT

say "Signing in as $user, inside the VM"
sign_in 443 || fail "cannot sign in as $user"
saved 443 "$default" >/dev/null || fail "the web interface's settings are not a fresh install's: $(body 443 GET /system/web-server)"
vm 'command -v openssl' >/dev/null || fail "no openssl in the VM"
http_before=$(conf http.conf)
https_before=$(conf https.conf)

say "1. Settings saved while confirming are in force again after a restart"
changed=yes
moved='{"http_port":80,"https_port":8443,"certificate_id":null}'
change 443 8443 "$moved"
saved 8443 "$moved" >/dev/null || fail "not saved: $(body 8443 GET /system/web-server)"
https_moved=$(conf https.conf)
[[ $https_moved == *"listen 8443 ssl"* ]] || fail "https.conf does not say 8443: $https_moved"
# As if the agent had gone back on its own: the files from before, served.
vm "sudo tee /etc/zendiloos/web/https.conf >/dev/null && sudo systemctl reload nginx" <<<"$https_before"
vm "sudo tee /etc/zendiloos/web/http.conf >/dev/null && sudo systemctl reload nginx" <<<"$http_before"
# nginx reloads in the background: new workers take over a moment later.
wait_for "nginx at 443 again, no longer at 8443" "serves 443 && ! serves 8443" 30
vm 'sudo systemctl restart zendiloosd'
wait_for "the settings saved, in force again" "serves 8443" 120
[[ $(conf https.conf) == "$https_moved" ]] || fail "https.conf is not what the settings saved say: $(conf https.conf)"
wait_for "nginx no longer at 443" "! serves 443" 30
no_previous || fail "files from before are left: $(vm 'ls /etc/zendiloos/web')"
sign_in 8443 || fail "cannot sign in at 8443"
saved 8443 "$moved" >/dev/null || fail "the settings saved changed: $(body 8443 GET /system/web-server)"
change 8443 443 "$default"

say "2. Two confirmations at once: one is taken, and the settings saved agree with the files"
other_http='{"http_port":8081,"https_port":443,"certificate_id":null}'
reply=$(inside 443 PUT /system/web-server "$other_http")
[[ $(tail -1 <<<"$reply") == 202 ]] || fail "applying: $reply"
both=$(vm "for _ in 1 2; do
        curl -sk -m 30 -o /dev/null -w '%{http_code}\n' -b $jar -X POST -H 'Origin: https://127.0.0.1' \
            -H \"X-CSRF-Token: \$(cat $csrf)\" https://127.0.0.1/api/v1/system/web-server/confirm &
    done; wait" | sort | tr '\n' ' ')
[[ $both == "204 409 " ]] || fail "two confirmations at once answered $both"
saved 443 "$other_http" >/dev/null || fail "the settings saved: $(body 443 GET /system/web-server)"
[[ $(conf http.conf) == *"listen 8081 default_server"* ]] || fail "http.conf: $(conf http.conf)"
no_previous || fail "files from before are left: $(vm 'ls /etc/zendiloos/web')"
change 443 443 "$default"
saved 443 "$default" >/dev/null || fail "back to the settings before: $(body 443 GET /system/web-server)"

say "3. A chain acme.sh hands over is checked whole; one that does not work leaves the pair served"
stand_in=yes
stand_in
reply=$(inside 443 POST /certificates/accounts \
    '{"name":"web-server-test","server":"letsencrypt_staging","email":"admin@example.org"}')
[[ $(tail -1 <<<"$reply") == 201 ]] || fail "creating the ACME account: $reply"
acme_account=$(sed '$d' <<<"$reply" | jq -r .id)
reply=$(inside 443 POST /certificates/challenges '{"name":"web-server-test","method":"http"}')
[[ $(tail -1 <<<"$reply") == 201 ]] || fail "creating the challenge: $reply"
challenge=$(sed '$d' <<<"$reply" | jq -r .id)
reply=$(inside 443 POST /certificates "$(jq -cn --arg d "$domain" --argjson a "$acme_account" \
    --argjson c "$challenge" '{domains: [$d], account_id: $a, challenge_id: $c}')")
[[ $(tail -1 <<<"$reply") == 201 ]] || fail "creating the certificate: $reply"
certificate=$(sed '$d' <<<"$reply" | jq -r .id)
chain leaf-2.pem ca.pem
job=$(issue)
[[ $(jq -r .state <<<"$job") == succeeded ]] || fail "a good chain: $job"
[[ $(serial_in_place) == serial=02 ]] || fail "the pair in place: $(serial_in_place)"
pair=$(pair_in_place)
chain leaf-3.pem junk.pem
job=$(issue)
[[ $(jq -r .state <<<"$job") == failed && $(jq -r .error_detail <<<"$job") == *"cannot be read"* ]] \
    || fail "a chain with a block that is no certificate: $job"
[[ $(pair_in_place) == "$pair" ]] || fail "the pair served changed with a broken chain"
chain leaf-3.pem other.pem
job=$(issue)
[[ $(jq -r .state <<<"$job") == failed && $(jq -r .error_detail <<<"$job") == *"not issued by the one after it"* ]] \
    || fail "a chain whose certificates do not belong together: $job"
[[ $(pair_in_place) == "$pair" ]] || fail "the pair served changed with a foreign certificate"
vm "! sudo test -e $served/$certificate.zendiloos-new" || fail "a pair checked is left beside the one served"
vm 'sudo nginx -t -q' || fail "nginx -t fails"

say "4. A renewal nginx does not take is told so, and taken at the next daily look"
change 443 443 "$(jq -cn --argjson c "$certificate" '{http_port: 80, https_port: 443, certificate_id: $c}')"
[[ $(serial_served) == serial=02 ]] || fail "nginx serves $(serial_served), not the certificate issued"
since=$(vm 'date +%s')
vm "sudo mkdir -p ${dropin%/*} && printf '[Service]\nExecReload=\nExecReload=/bin/false\n' | sudo tee $dropin >/dev/null \
    && sudo systemctl daemon-reload"
chain leaf-3.pem ca.pem
job=$(issue)
[[ $(jq -r .state <<<"$job") == failed && $(jq -r .error_detail <<<"$job") == *"did not take"* ]] \
    || fail "issuing while nginx cannot reload: $job"
[[ $(serial_in_place) == serial=03 ]] || fail "the new pair is not in place: $(serial_in_place)"
[[ $(serial_served) == serial=02 ]] || fail "nginx serves $(serial_served), not the one before"
body 443 GET /certificates | jq -e --argjson id "$certificate" \
    '.[] | select(.id == $id) | .last_error | contains("did not take")' >/dev/null \
    || fail "the certificate does not show why: $(body 443 GET /certificates)"
told certificate_not_taken "$since" >/dev/null || fail "no notification that nginx did not take it"
vm "sudo rm -f $dropin && sudo systemctl daemon-reload"
vm 'sudo systemctl restart zendiloosd'
# The session outlives the restart; nginx limits sign-ins, not this.
wait_for "zendiloosd after the restart" "serves 443" 120
say "   waiting for the daily look, five minutes after the start"
wait_for "the daily look" "body 443 GET /certificates | jq -e --argjson id $certificate \
    '.[] | select(.id == \$id) | .last_error == null'" 420
[[ $(serial_served) == serial=03 ]] || fail "nginx serves $(serial_served) after the daily look"
told certificate_renewed "$since" >/dev/null || fail "no notification that it was renewed"

say "The web interface's settings and certificates work."
