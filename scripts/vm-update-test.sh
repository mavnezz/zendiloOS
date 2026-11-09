#!/usr/bin/env bash
# Tests updates end to end in a test VM:
#
#   1. a check through the API: apt's lists fetched in a unit of its own,
#      what Debian would upgrade reported (GitHub is asked too)
#   2. Debian's updates installed through the API, if there are any (a scrub
#      tried meanwhile is refused with update_running), and the ZFS module
#      checked for the newest kernel
#   3. zendiloOS updates itself to a newer release, signed with a test key:
#      the runner copies the files out of the download folder, checks the
#      signature, that it was made for this version, the channel, the
#      checksum and the version, installs, and zendiloosd comes back with
#      the new version
#   4. a changed package, a signature made for another version, a test or
#      development build on the stable channel and an older release are
#      refused
#   5. an update cut off while dpkg configured (a package whose postinst
#      fails until it may succeed) is found by a check and finished
#      through the API
#
# Usage: scripts/vm-update-test.sh NEXT_DEB [CURRENT_DEB]
#   NEXT_DEB     a package of a newer version than the installed one, e.g.
#                ZENDILOOS_VERSION=0.1.1~vmtest1 just deb
#   CURRENT_DEB  the installed version's package (for the refused downgrade
#                and to put the VM back); the newest in dist/ by default
#
# Needs the VM running with the package installed and set up, and the
# network for apt. Runs in the second VM by default (the first holds a LAN
# test): it upgrades Debian there. minisign is installed in the VM for the
# test key, which goes away afterwards.
set -euo pipefail

export ZENDILOOS_VM=${ZENDILOOS_VM:-2}
root=$(cd "$(dirname "$0")/.." && pwd)
next=${1:?usage: $0 NEXT_DEB [CURRENT_DEB]}
current=${2:-$(ls -t "$root"/dist/zendiloos_*_amd64.deb | head -1)}
base="https://127.0.0.1:${ZENDILOOS_HTTPS_PORT:-$((8442 + ZENDILOOS_VM))}"
api="$base/api/v1"
user=${ZENDILOOS_USER:-admin}
password=${ZENDILOOS_PASSWORD:-correct horse battery}
spool=/var/lib/zendiloos/updates
work=$(mktemp -d)

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }
vm() { "$root/scripts/vm.sh" ssh "$@"; }

sign_in() {
    curl -sk -c "$work/admin.jar" -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg name "$user" --arg password "$password" '{name: $name, password: $password}')" \
        "$api/auth/login" | jq -er .csrf_token >"$work/admin.csrf"
}

call() {
    local method=$1 path=$2 body=${3:-}
    local args=(-sk -b "$work/admin.jar" -c "$work/admin.jar" -X "$method" -H "Origin: $base"
                -H "X-CSRF-Token: $(cat "$work/admin.csrf" 2>/dev/null)")
    [[ -n $body ]] && args+=(-H 'Content-Type: application/json' -d "$body")
    curl "${args[@]}" "$api$path"
}

# The HTTP status of a request without a body.
status() {
    curl -sk -o /dev/null -w '%{http_code}' -b "$work/admin.jar" -X "$1" -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/admin.csrf")" "$api$2"
}

wait_for() {
    local what=$1 check=$2 seconds=${3:-120}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

version_of() { basename "$1" | sed 's/^zendiloos_\(.*\)_amd64\.deb$/\1/'; }

# The steadiest channel that takes a version, as the runner tells it.
channel_of() {
    case $1 in
        *+*) echo development ;;
        *~*) echo test ;;
        *) echo stable ;;
    esac
}

# Puts a package with SHA256SUMS signed by the test key into the download
# folder; the signature is made for the package's version, as CI makes it,
# or for what the second argument says.
offer() {
    local deb=$1 name for
    name=$(basename "$deb")
    for=${2:-zendiloos $(version_of "$deb")}
    vm "sudo rm -rf $spool && sudo install -d -o zendiloos -g zendiloos -m 0750 $spool"
    vm "sudo tee $spool/$name >/dev/null" <"$deb"
    vm "cd /tmp/zdl-key && sudo sh -c 'cd $spool && sha256sum $name > SHA256SUMS' \
        && sudo cp $spool/SHA256SUMS . && minisign -S -s key -m SHA256SUMS -t '$for' >/dev/null \
        && sudo cp SHA256SUMS.minisig $spool/ && sudo chown -R zendiloos:zendiloos $spool"
}

# Runs the update as the agent starts it, for the channel that takes the
# package or the one given; prints the run's record when done.
run_update() {
    local name=$1 id channel
    channel=${2:-$(channel_of "$(version_of "$name")")}
    id=$(od -An -tx1 -N8 /dev/urandom | tr -d ' \n')
    vm "sudo systemd-run --unit=zendiloos-packages --collect --quiet --property=Type=exec \
        /usr/libexec/zendiloos/zendiloos-agent run-packages $id update $name $channel" >/dev/null
    wait_for "the run" "vm 'cat /var/lib/zendiloos-packages/run.json' | jq -e --arg id $id '.id == \$id and .state != \"running\"'" 1800
    vm 'cat /var/lib/zendiloos-packages/run.json'
}

cleanup() {
    set +e
    vm "sudo rm -rf /etc/zendiloos/release-signing.pub /tmp/zdl-key $spool" 2>/dev/null
    vm 'sudo touch /run/zdl-half-done-may; sudo dpkg --purge zdl-half-done >/dev/null 2>&1;
        sudo rm -rf /run/zdl-half-done-may /tmp/zdl-half /tmp/zdl-half-done.deb' 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT

installed=$(vm "dpkg-query -W -f '\${Version}' zendiloos")
next_version=$(version_of "$next")
current_version=$(version_of "$current")
say "Signing in as $user (zendiloOS $installed installed, $next_version offered)"
sign_in || fail "cannot sign in"

say "1. A check: apt's lists and GitHub"
# The daily check may be going on already, a while after zendiloosd started.
wait_for "a check to start" "[[ \$(status POST /system/updates/check) == 202 ]]" 900
wait_for "the check" "call GET /system/updates | jq -e '.phase == \"idle\" and .checked_at != null'" 900
overview=$(call GET /system/updates)
jq -e '.packages_error == null' <<<"$overview" >/dev/null || fail "apt: $(jq -r .packages_error <<<"$overview")"
count=$(jq '.packages | length' <<<"$overview")
echo "  Debian: $count packages ($(jq '[.packages[] | select(.security)] | length' <<<"$overview") security);" \
    "GitHub: $(jq -r '.release.version // .release_error // "nothing newer"' <<<"$overview")"

say "2. Debian's updates through the API"
if (( count > 0 )) && jq -e '.release == null' <<<"$overview" >/dev/null; then
    [[ $(status POST /system/updates/apply) == 202 ]] || fail "apply not started"
    # While it installs no job starts, nor a scrub: refused, not queued.
    pool=$(call GET /storage/pools | jq -r '[.[] | select(.volume != null)][0].name // empty')
    if [[ -n $pool ]]; then
        tried=
        for _ in $(seq 1 120); do
            if call GET /system/updates | jq -e '.phase == "downloading" or .run.state == "running"' >/dev/null; then
                tried=$(call POST "/storage/pools/$pool/scrub")
                break
            fi
            sleep 1
        done
        if [[ -n $tried ]]; then
            jq -e '.code == "update_running"' <<<"$tried" >/dev/null || fail "a scrub started while updating: $tried"
            echo "  a scrub of $pool was refused while the update installed"
        else
            echo "  the update was over before a scrub could be tried"
        fi
    fi
    wait_for "the update" "call GET /system/updates | jq -e '.run != null and .run.state != \"running\"'" 1800
    run=$(call GET /system/updates | jq .run)
    jq -e '.state == "succeeded" and .zfs_ready' <<<"$run" >/dev/null || fail "the update: $run"
    after=$(call GET /system/updates | jq '.packages | length')
    echo "  installed; $after left; restart needed: $(jq .restart_needed <<<"$run")"
else
    echo "  skipped: nothing from Debian, or a release would come along"
fi

say "3. zendiloOS updates itself to $next_version"
vm 'command -v minisign >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y install minisign >/dev/null'
vm 'rm -rf /tmp/zdl-key && mkdir /tmp/zdl-key && cd /tmp/zdl-key && minisign -G -W -p pub -s key >/dev/null'
vm 'sudo install -m 0644 /tmp/zdl-key/pub /etc/zendiloos/release-signing.pub'

name=$(basename "$next")
offer "$next"
vm "echo changed | sudo tee -a $spool/$name >/dev/null"
record=$(run_update "$name")
jq -e '.state == "failed" and (.error | contains("checksum"))' <<<"$record" >/dev/null \
    || fail "a changed package was taken: $record"

# A good signature, but made for another build.
offer "$next" "zendiloos $current_version"
record=$(run_update "$name")
jq -e '.state == "failed" and (.error | contains("signature is for"))' <<<"$record" >/dev/null \
    || fail "a signature made for $current_version was taken for $next_version: $record"

if [[ $(channel_of "$next_version") != stable ]]; then
    offer "$next"
    record=$(run_update "$name" stable)
    jq -e '.state == "failed" and (.error | contains("stable channel does not take"))' <<<"$record" >/dev/null \
        || fail "the stable channel took $next_version: $record"
fi

offer "$next"
record=$(run_update "$name")
jq -e --arg v "$next_version" '.state == "succeeded" and .release == $v' <<<"$record" >/dev/null \
    || fail "the update: $record"
[[ $(vm "dpkg-query -W -f '\${Version}' zendiloos") == "$next_version" ]] || fail "zendiloos is not $next_version"
wait_for "the API" "sign_in" 120
[[ $(call GET /system/info | jq -r .version) == "$next_version" ]] || fail "zendiloosd reports $(call GET /system/info | jq -r .version)"
call GET /system/updates | jq -e --arg v "$next_version" '.current == $v and .run.zendiloos == $v and .run.state == "succeeded"' \
    >/dev/null || fail "the overview: $(call GET /system/updates | jq -c '{current, run}')"

say "4. An older release is refused"
offer "$current"
record=$(run_update "$(basename "$current")")
jq -e '.state == "failed" and (.error | contains("not newer"))' <<<"$record" >/dev/null \
    || fail "an older release was taken: $record"

say "5. An update cut off while dpkg configured is finished through the API"
# A package that cannot be configured until it may: dpkg leaves it half
# configured, as a power cut in the middle of an update would.
vm 'rm -rf /tmp/zdl-half && mkdir -p /tmp/zdl-half/DEBIAN \
    && printf "Package: zdl-half-done\nVersion: 1.0\nArchitecture: all\nMaintainer: Test <test@example.org>\nDescription: configures once it may\n" \
        >/tmp/zdl-half/DEBIAN/control \
    && printf "#!/bin/sh\n[ -e /run/zdl-half-done-may ] || exit 1\n" >/tmp/zdl-half/DEBIAN/postinst \
    && chmod 0755 /tmp/zdl-half/DEBIAN/postinst \
    && dpkg-deb --root-owner-group --build /tmp/zdl-half /tmp/zdl-half-done.deb >/dev/null'
vm 'sudo dpkg -i /tmp/zdl-half-done.deb >/dev/null 2>&1' && fail "zdl-half-done was configured although it may not"
[[ -n $(vm 'dpkg --audit') ]] || fail "dpkg lists nothing half done"
wait_for "a check to start" "[[ \$(status POST /system/updates/check) == 202 ]]" 900
wait_for "the check" "call GET /system/updates | jq -e '.phase == \"idle\" and .interrupted'" 900
vm 'sudo touch /run/zdl-half-done-may'
[[ $(status POST /system/updates/finish) == 202 ]] || fail "finishing did not start"
wait_for "finishing" "call GET /system/updates | jq -e '.run.finishes and .run.state != \"running\"'" 1800
run=$(call GET /system/updates | jq .run)
jq -e '.state == "succeeded"' <<<"$run" >/dev/null || fail "finishing: $run"
[[ -z $(vm 'dpkg --audit') ]] || fail "dpkg still lists: $(vm 'dpkg --audit')"
call GET /system/updates | jq -e '.interrupted == false' >/dev/null || fail "the page still offers to finish"
vm 'sudo dpkg --purge zdl-half-done >/dev/null && sudo rm -f /run/zdl-half-done-may'

say "All update tests passed (put the VM back with: ZENDILOOS_VM=$ZENDILOOS_VM scripts/vm.sh install $current)"
