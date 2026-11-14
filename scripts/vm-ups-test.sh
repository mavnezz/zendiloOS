#!/usr/bin/env bash
# Tests following a UPS end to end in a test VM, through the HTTPS API,
# with Network UPS Tools' own programs inside the VM. No UPS can be plugged
# into the VM: its place is taken by NUT's dummy-ups (the source
# "simulated", which the API alone can set) and, for "another device", by a
# second NUT server started beside the NAS's own.
#
#   1. Network UPS Tools are installed by the page's job; nothing of them
#      runs until a UPS is set
#   2. settings the API refuses change nothing
#   3. a UPS on this NAS: NUT's files (for root and the group nut alone),
#      the driver, the server (on this machine only) and the monitor run,
#      the API shows what the UPS says; the same settings again restart
#      nothing
#   4. a power cut and its end are told
#   5. other devices may follow: the server listens on the network, their
#      account signs in, a wrong password does not; the password is kept
#      when left out and never shown
#   6. a charge to shut down at: on battery above it the NAS stays up,
#      below it the VM powers off; started again, NUT follows the UPS by
#      itself
#   7. another device's UPS: no driver and no server here, its state read
#      from there; after the minutes on battery the VM powers off
#   8. started again without the other device, the UPS is told as not
#      answering
#   9. no UPS: NUT is switched off
#
# Needs the VM running with the package installed and set up; account and
# password as for vm-storage-test.sh. Runs in the second VM by default (the
# first holds a LAN test). The VM powers off twice and is started again
# (scripts/vm.sh start); a VM that holds a guest which ignores its shutdown
# takes minutes longer each time.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
# Where the "other device" answers: a second upsd on this port.
other_port=3494
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

# The HTTP status of a request; what it answered is in $work/answer.
status_of() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -o "$work/answer" -w '%{http_code}' -b "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf")")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-30}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

sign_in() {
    curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf"
}

ups() { call GET /ups; }
# Sets the UPS; fails unless the API took it.
put() {
    [[ $(status_of PUT /ups "$1") == 200 ]] || fail "not set ($1): $(cat "$work/answer")"
}
# How many notifications of a kind there are.
told() {
    call GET '/notifications?limit=100' | jq --arg code "$1" '[.notifications[] | select(.code == $code)] | length'
}
# When a unit last started, as systemd counts.
started() { vm "systemctl show -p ActiveEnterTimestampMonotonic --value $1"; }
# The addresses NUT's server listens on.
listening() { vm "sudo ss -Hltn 'sport = :3493'" | awk '{print $4}'; }

# The simulated UPS is told what to say, as its account may.
play() {
    local secret
    secret=$(vm "sudo sed -n '/^\[upsmon_local\]/,/^\[/s/^ *password = //p' /etc/nut/upsd.users")
    for setting in "$@"; do
        vm "upsrw -s '$setting' -u upsmon_local -p '$secret' ups@localhost" >/dev/null 2>&1 \
            || fail "the simulated UPS did not take $setting"
    done
}

# What a device that follows does first: sign in for the UPS. Prints NUT's
# answers, a line each.
follow() {
    local address=$1 account=$2 secret=$3
    vm "bash -s" <<EOF
exec 3<>/dev/tcp/$address/3493
printf 'USERNAME %s\nPASSWORD %s\nLOGIN ups\nLOGOUT\n' '$account' '$secret' >&3
timeout 5 cat <&3 || true
EOF
}

vm_dir=$root/dev/vm
[[ $ZENDILOOS_VM == 1 ]] || vm_dir=$root/dev/vm$ZENDILOOS_VM
powered() { [[ -f $vm_dir/qemu.pid ]] && kill -0 "$(cat "$vm_dir/qemu.pid")" 2>/dev/null; }
# The VM switched off by NUT, then on again as when the power returns.
off_and_on_again() {
    local why=$1
    wait_for "the VM to power off ($why)" "! powered" 600
    say "   powered off; starting it again"
    "$root/scripts/vm.sh" start >/dev/null
    wait_for "zendiloosd after the start" "curl -skf $api/health" 300
    wait_for "to sign in again" sign_in 60
}

# A second NUT server beside the NAS's own, with a simulated UPS of its
# own and an account for those that follow: the "other device". Its files
# and state are apart from the NAS's (NUT_CONFPATH, NUT_STATEPATH).
other_device() {
    vm "sudo bash -s" <<EOF
set -e
systemctl stop zendiloos-test-upsd zendiloos-test-upsdrv 2>/dev/null || true
rm -rf /etc/nut-other /run/nut-other
install -d -o root -g nut -m 0750 /etc/nut-other
install -d -o nut -g nut -m 0770 /run/nut-other
printf '[ups]\n    driver = dummy-ups\n    port = other.dev\n' >/etc/nut-other/ups.conf
printf 'ups.status: OL\nbattery.charge: 90\nbattery.runtime: 1200\nups.mfr: zendiloOS\nups.model: Other device\n' \
    >/etc/nut-other/other.dev
printf 'LISTEN 127.0.0.1 $other_port\n' >/etc/nut-other/upsd.conf
printf '[follower]\n    password = follows\n    upsmon secondary\n[player]\n    password = plays\n    actions = SET\n    instcmds = ALL\n' \
    >/etc/nut-other/upsd.users
chown root:nut /etc/nut-other/*
chmod 0640 /etc/nut-other/*
env='-E NUT_CONFPATH=/etc/nut-other -E NUT_STATEPATH=/run/nut-other -E NUT_ALTPIDPATH=/run/nut-other'
systemd-run -q --unit=zendiloos-test-upsdrv \$env /lib/nut/dummy-ups -F -a ups
systemd-run -q --unit=zendiloos-test-upsd \$env /usr/sbin/upsd -F
EOF
}
play_other() {
    for setting in "$@"; do
        vm "upsrw -s '$setting' -u player -p plays ups@localhost:$other_port" >/dev/null 2>&1 \
            || fail "the other device's UPS did not take $setting"
    done
}

finish() {
    set +e
    if powered && sign_in 2>/dev/null; then
        call PUT /ups '{"source":null,"shutdown":{"kind":"low_battery"}}' >/dev/null
        vm "sudo systemctl stop zendiloos-test-upsd zendiloos-test-upsdrv 2>/dev/null
            sudo rm -rf /etc/nut-other /run/nut-other" 2>/dev/null
    fi
    rm -rf "$work"
}
trap finish EXIT

say "Signing in as $user"
sign_in || fail "cannot sign in"
[[ $(ups | jq -r '.settings.source') == null ]] || fail "a UPS is set already: $(ups | jq -c .settings)"

say "1. Network UPS Tools from the page"
if ups | jq -e '.installed and .snmp' >/dev/null; then
    say "   (installed already)"
else
    job=$(call POST /ups/install | jq -er .id) || fail "the installation did not start"
    wait_for "the installation" "[[ \$(call GET /jobs/$job | jq -r .state) != running ]]" 900
    [[ $(call GET "/jobs/$job" | jq -r .state) == succeeded ]] || fail "the installation: $(call GET "/jobs/$job")"
    ups | jq -e '.installed and .snmp' >/dev/null || fail "installed, the page says: $(ups)"
    call GET /logs/audit | jq -e '[.. | strings | select(test("ups.install"))] | length > 0' >/dev/null \
        || fail "the installation is not in the audit log"
fi
[[ $(status_of POST /ups/install) == 409 ]] || fail "installed a second time: $(cat "$work/answer")"
ups | jq -e '(.drivers | index("usbhid-ups")) != null and .name == "ups" and .port == 3493' >/dev/null \
    || fail "the page's facts: $(ups)"
# Debian installs NUT switched off, and without a UPS zendiloOS leaves it so.
for unit in nut-server nut-monitor; do
    [[ $(vm "systemctl is-active $unit" || true) != active ]] || fail "$unit runs without a UPS"
done
vm "sudo grep -q '^MODE=none' /etc/nut/nut.conf" || fail "nut.conf: $(vm 'sudo grep ^MODE /etc/nut/nut.conf')"
[[ $(status_of POST /ups/scan) == 200 ]] || fail "looking for UPSs on USB: $(cat "$work/answer")"
jq -e '.devices | type == "array"' "$work/answer" >/dev/null || fail "the scan answered: $(cat "$work/answer")"

say "2. Settings the API refuses change nothing"
low='"shutdown":{"kind":"low_battery"}'
for body in \
    "{\"source\":{\"kind\":\"usb\",\"driver\":\"no-such-driver\"},$low}" \
    '{"source":{"kind":"simulated"},"shutdown":{"kind":"charge_below","percent":2}}' \
    '{"source":{"kind":"simulated"},"shutdown":{"kind":"after_minutes","minutes":0}}' \
    "{\"source\":{\"kind\":\"snmp\",\"address\":\"10.0.0.9; reboot\",\"community\":\"public\",\"version\":\"v2c\"},$low}" \
    "{\"source\":{\"kind\":\"server\",\"host\":\"nas.local\",\"user\":\"monuser\"},$low}" \
    "{\"source\":{\"kind\":\"server\",\"host\":\"nas.local\",\"user\":\"monuser\",\"password\":\"x\"},$low,\"cut_power\":true}" \
    "{\"source\":{\"kind\":\"simulated\"},$low,\"serve\":{\"user\":\"upsmon_local\",\"password\":\"x\"}}" \
    "{\"source\":null,$low,\"serve\":{\"user\":\"monuser\",\"password\":\"x\"}}"; do
    [[ $(status_of PUT /ups "$body") == 422 ]] || fail "not refused ($body): $(cat "$work/answer")"
done
[[ $(ups | jq -r '.settings.source') == null ]] || fail "a refused UPS was kept"
vm "sudo grep -q '^MODE=none' /etc/nut/nut.conf" || fail "a refused UPS reached nut.conf"

say "3. A UPS on this NAS"
put "{\"source\":{\"kind\":\"simulated\"},$low}"
wait_for "the UPS to answer" "ups | jq -e '.reading.status == \"OL\"'" 60
ups | jq -e '.reading | (.on_battery | not) and (.low_battery | not) and .charge_percent == 100
    and .runtime_seconds == 1800 and .load_percent == 20 and .model == "Simulated UPS"' >/dev/null \
    || fail "the reading: $(ups | jq -c .reading)"
for file in nut.conf ups.conf upsd.conf upsd.users upsmon.conf upssched.conf; do
    [[ $(vm "sudo stat -c '%a %U %G' /etc/nut/$file") == '640 root nut' ]] \
        || fail "$file is $(vm "sudo stat -c '%a %U %G' /etc/nut/$file")"
done
vm "sudo grep -q '^MODE=standalone' /etc/nut/nut.conf" || fail "nut.conf is not standalone"
vm "sudo test -f /etc/nut/nut.conf.before-zendiloos" || fail "Debian's nut.conf was not kept beside"
for unit in nut-driver@ups nut-server nut-monitor; do
    [[ $(vm "systemctl is-active $unit") == active ]] || fail "$unit does not run"
done
for unit in nut-server nut-monitor; do
    [[ $(vm "systemctl is-enabled $unit") == enabled ]] || fail "$unit is not enabled"
done
[[ $(listening) == 127.0.0.1:3493 ]] || fail "NUT's server listens on: $(listening)"
[[ $(vm "upsc ups@localhost ups.status 2>/dev/null") == OL ]] || fail "upsc says $(vm 'upsc ups@localhost ups.status')"
call GET /logs/audit | jq -e '[.. | strings | select(test("ups.update"))] | length > 0' >/dev/null \
    || fail "the change is not in the audit log"
# The same settings again, as at every start of zendiloosd: NUT keeps running.
server_started=$(started nut-server)
monitor_started=$(started nut-monitor)
put "{\"source\":{\"kind\":\"simulated\"},$low}"
vm "sudo systemctl restart zendiloosd"
wait_for "zendiloosd after its restart" "curl -skf $api/health" 60
sign_in || fail "cannot sign in after the restart"
sleep 5
[[ $(started nut-server) == "$server_started" && $(started nut-monitor) == "$monitor_started" ]] \
    || fail "NUT was restarted for settings that were in force"

say "4. A power cut and its end are told"
cut_before=$(told ups_on_battery)
back_before=$(told ups_on_mains)
play 'ups.status=OB DISCHRG'
wait_for "the power cut to be told" "(( \$(told ups_on_battery) > $cut_before ))" 60
ups | jq -e '.reading.on_battery and (.reading.low_battery | not)' >/dev/null \
    || fail "on battery, the reading says: $(ups | jq -c .reading)"
play 'ups.status=OL CHRG'
wait_for "the power's return to be told" "(( \$(told ups_on_mains) > $back_before ))" 60
ups | jq -e '(.reading.on_battery | not) and .reading.charging' >/dev/null \
    || fail "on mains, the reading says: $(ups | jq -c .reading)"
(( $(told ups_on_battery) == cut_before + 1 )) || fail "the power cut was told more than once"

say "5. Other devices follow the UPS through the NAS"
put "{\"source\":{\"kind\":\"simulated\"},$low,\"serve\":{\"user\":\"monuser\",\"password\":\"secret\"}}"
wait_for "the server on the network" "[[ \$(listening) == 0.0.0.0:3493 ]]" 30
vm "sudo grep -q '^MODE=netserver' /etc/nut/nut.conf" || fail "nut.conf is not netserver"
ups | jq -e '.settings.serve == {"user": "monuser", "password_set": true}' >/dev/null \
    || fail "the account as the API shows it: $(ups | jq -c .settings.serve)"
[[ $(ups) != *secret* ]] || fail "the API shows the account's password"
address=$(vm "ip -4 -o addr show scope global" | awk '{print $4}' | cut -d/ -f1 | head -n1)
[[ -n $address ]] || fail "the VM has no address"
wait_for "the server to answer on $address" "[[ \$(follow $address monuser secret) == OK* ]]" 30
answers=$(follow "$address" monuser secret)
[[ $(grep -c '^OK' <<<"$answers") == 4 ]] || fail "the account could not sign in for the UPS: $answers"
answers=$(follow "$address" monuser wrong)
grep -q 'ACCESS-DENIED' <<<"$answers" || fail "a wrong password signed in: $answers"
# The account may follow, not set.
if vm "upsrw -s 'ups.status=OB' -u monuser -p secret ups@$address" >/dev/null 2>&1; then
    fail "the account for other devices may set the UPS"
fi
# Changed without the password typed again: the one kept stays.
put "{\"source\":{\"kind\":\"simulated\"},\"shutdown\":{\"kind\":\"after_minutes\",\"minutes\":30},\"serve\":{\"user\":\"monuser\"}}"
wait_for "the server after the change" "[[ \$(follow $address monuser secret) == OK* ]]" 30
[[ $(follow "$address" monuser secret | grep -c '^OK') == 4 ]] || fail "the kept password no longer signs in"
vm "sudo grep -q 'START-TIMER onbatt 1800' /etc/nut/upssched.conf" || fail "no timer of 30 minutes in upssched.conf"
# Another account needs its password.
[[ $(status_of PUT /ups "{\"source\":{\"kind\":\"simulated\"},$low,\"serve\":{\"user\":\"other\"}}") == 422 ]] \
    || fail "an account without a password was taken: $(cat "$work/answer")"

say "6. Below the charge set, the NAS shuts down"
put '{"source":{"kind":"simulated"},"shutdown":{"kind":"charge_below","percent":40},"cut_power":true,"serve":{"user":"monuser"}}'
vm "sudo grep -q 'override.battery.charge.low = 40' /etc/nut/ups.conf && sudo grep -q ignorelb /etc/nut/ups.conf" \
    || fail "ups.conf: $(vm 'sudo cat /etc/nut/ups.conf')"
vm "sudo grep -q '^POWERDOWNFLAG /etc/killpower' /etc/nut/upsmon.conf" || fail "no POWERDOWNFLAG in upsmon.conf"
wait_for "the UPS to answer again" "ups | jq -e '.reading.status == \"OL\"'" 60
cut_before=$(told ups_on_battery)
play 'battery.charge=60' 'ups.status=OB DISCHRG'
wait_for "the power cut to be told" "(( \$(told ups_on_battery) > $cut_before ))" 60
sleep 20
ups | jq -e '.reading.on_battery and (.reading.low_battery | not) and .reading.charge_percent == 60' >/dev/null \
    || fail "above the charge, the reading says: $(ups | jq -c .reading)"
down_before=$(told ups_shutdown)
# The charge first: the simulated UPS weighs it when its status is set.
play 'battery.charge=30' 'ups.status=OB DISCHRG'
off_and_on_again "the charge fell below 40 %"
(( $(told ups_shutdown) > down_before )) || fail "the shutdown was not told before it came"
for unit in nut-driver@ups nut-server nut-monitor; do
    wait_for "$unit after the start" "[[ \$(vm 'systemctl is-active $unit') == active ]]" 60
done
# The simulated UPS starts on mains again, as a real one once the power is back.
wait_for "the UPS to answer after the start" "ups | jq -e '.reading.status == \"OL\"'" 60
ups | jq -e '.settings | .cut_power and .shutdown == {"kind": "charge_below", "percent": 40}' >/dev/null \
    || fail "the settings after the start: $(ups | jq -c .settings)"

say "7. The UPS of another device"
other_device
wait_for "the other device's server" "[[ \$(vm 'upsc ups@localhost:$other_port ups.status 2>/dev/null') == OL ]]" 30
put "{\"source\":{\"kind\":\"server\",\"host\":\"127.0.0.1\",\"port\":$other_port,\"ups\":\"ups\",\"user\":\"follower\",\"password\":\"follows\"},\"shutdown\":{\"kind\":\"after_minutes\",\"minutes\":1}}"
vm "sudo grep -q '^MODE=netclient' /etc/nut/nut.conf" || fail "nut.conf is not netclient"
vm "sudo grep -q '^MONITOR ups@127.0.0.1:$other_port 1 follower follows secondary' /etc/nut/upsmon.conf" \
    || fail "upsmon.conf does not follow the other device"
[[ $(vm "systemctl is-active nut-monitor") == active ]] || fail "the monitor does not run"
for unit in nut-server nut-driver@ups; do
    [[ $(vm "systemctl is-active $unit" || true) != active ]] || fail "$unit runs for another device's UPS"
done
[[ -z $(listening) ]] || fail "NUT's server still listens: $(listening)"
wait_for "the other device's UPS in the API" "ups | jq -e '.reading.model == \"Other device\"'" 60
ups | jq -e '.settings.source | .password_set and (has("password") | not)' >/dev/null \
    || fail "the source as the API shows it: $(ups | jq -c .settings.source)"
cut_before=$(told ups_on_battery)
began=$(date +%s)
play_other 'ups.status=OB DISCHRG'
wait_for "the other device's power cut to be told" "(( \$(told ups_on_battery) > $cut_before ))" 60
off_and_on_again "a minute on battery"
took=$(( $(date +%s) - began ))
say "   (the VM was back $took seconds after the power cut)"

say "8. A UPS that no longer answers is told"
silent_before=$(told ups_unreachable)
# The other device is gone with the restart: nothing answers on its port.
ups | jq -e '.settings.source.kind == "server" and .reading == null' >/dev/null \
    || fail "without the other device, the page says: $(ups | jq -c '{source: .settings.source, reading}')"
wait_for "the silence to be told" "(( \$(told ups_unreachable) > $silent_before ))" 120

say "9. No UPS: NUT is switched off"
put "{\"source\":null,$low}"
for unit in nut-server nut-monitor nut-driver@ups; do
    [[ $(vm "systemctl is-active $unit" || true) != active ]] || fail "$unit still runs"
done
for unit in nut-server nut-monitor; do
    [[ $(vm "systemctl is-enabled $unit" || true) != enabled ]] || fail "$unit is still enabled"
done
vm "sudo grep -q '^MODE=none' /etc/nut/nut.conf" || fail "nut.conf: $(vm 'sudo grep ^MODE /etc/nut/nut.conf')"
vm "! sudo grep -rqs 'secret\|follows' /etc/nut/upsd.users /etc/nut/upsmon.conf" || fail "passwords left in NUT's files"
ups | jq -e '.settings.source == null and .reading == null' >/dev/null || fail "the page says: $(ups)"

say "All checks passed"
