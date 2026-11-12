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
#   6. a second kernel, newer than the running one, installed without its
#      headers, so DKMS builds no ZFS for it (or with its module removed
#      should it have been built): GRUB's default stays on the running
#      kernel also once GRUB's configuration lists the new one first, a
#      check says so, the default moves to the new kernel once a module is
#      there and back once it is gone, and stays when the kernel goes again
#      (the kernel from ZENDILOOS_TEST_KERNEL, a package name, else the
#      newest of the running kernel's flavour in trixie-backports)
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

# GRUB's default, as saved in its environment block.
grub_default() { vm 'sudo grub-editenv /boot/grub/grubenv list' | sed -n 's/^saved_entry=//p'; }
# The id of the entry that starts kernel $1 and nothing else.
grub_entry_of() { printf '>gnulinux-%s-advanced-' "$1"; }
# Whether dpkg orders version $1 after $2.
newer_than() { vm "dpkg --compare-versions '$1' gt '$2'"; }
# Runs a check through the API and waits for its end.
check_now() {
    local before
    before=$(call GET /system/updates | jq '.checked_at // 0')
    wait_for "a check to start" "[[ \$(status POST /system/updates/check) == 202 ]]" 900
    wait_for "the check" "call GET /system/updates | jq -e '.phase == \"idle\" and .checked_at > $before'" 900
}
test_kernel=""

# Purges the test kernel and every package of its version: Debian's kernels
# from 7.x come as linux-image, linux-binary and linux-modules (with their
# headers and kbuild), and purging linux-image alone leaves the others, and
# the kernel's modules, behind.
purge_kernel() {
    local base=${1#linux-image-} installed packages
    base=${base%%-*}
    installed=$(vm "dpkg-query -W -f='\${Package} \${db:Status-Abbrev}\n'")
    packages=$(awk -v base="-$base" '$2 ~ /^[ir]/ && index($1, base) {print $1}' <<<"$installed" | tr '\n' ' ')
    [[ -z $packages ]] || vm "sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y purge $packages >/dev/null 2>&1"
}

cleanup() {
    set +e
    if [[ -n $test_kernel ]]; then
        purge_kernel "$test_kernel"
    fi
    vm 'if [ -e /etc/apt/sources.list.d/zdl-backports.list ]; then
            sudo rm -f /etc/apt/sources.list.d/zdl-backports.list && sudo apt-get -q update >/dev/null 2>&1
        fi' 2>/dev/null
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
[[ -n $(vm 'sudo dpkg --audit') ]] || fail "dpkg lists nothing half done"
wait_for "a check to start" "[[ \$(status POST /system/updates/check) == 202 ]]" 900
wait_for "the check" "call GET /system/updates | jq -e '.phase == \"idle\" and .interrupted'" 900
vm 'sudo touch /run/zdl-half-done-may'
[[ $(status POST /system/updates/finish) == 202 ]] || fail "finishing did not start"
wait_for "finishing" "call GET /system/updates | jq -e '.run.finishes and .run.state != \"running\"'" 1800
run=$(call GET /system/updates | jq .run)
jq -e '.state == "succeeded"' <<<"$run" >/dev/null || fail "finishing: $run"
# Root only: dpkg checks its lock first and says nothing to others.
[[ -z $(vm 'sudo dpkg --audit') ]] || fail "dpkg still lists: $(vm 'sudo dpkg --audit')"
call GET /system/updates | jq -e '.interrupted == false' >/dev/null || fail "the page still offers to finish"
vm 'sudo dpkg --purge zdl-half-done >/dev/null && sudo rm -f /run/zdl-half-done-may'

say "6. A newer kernel without its ZFS module is not started"
vm "grep -qF 'set default=\"\${saved_entry}\"' /boot/grub/grub.cfg" \
    || fail "GRUB's configuration does not start the saved entry: $(vm 'cat /etc/default/grub.d/zendiloos.cfg' 2>&1)"
running=$(vm 'uname -r')
# The newest kernel with its module: the running one, or one step 2 brought.
check_now
overview=$(call GET /system/updates)
kept=$(jq -r 'select(.boot.reason == "newest") | .boot.kernel' <<<"$overview")
[[ -n $kept ]] || fail "GRUB does not start the newest kernel: $(jq -c .boot <<<"$overview")"
[[ $(grub_default) == *"$(grub_entry_of "$kept")"* ]] || fail "GRUB's default is not $kept: $(grub_default)"
test_kernel=${ZENDILOOS_TEST_KERNEL:-}
if [[ -z $test_kernel ]]; then
    # 6.12.57+deb13-cloud-amd64: cloud-amd64
    flavour=${running#*-}
    vm 'echo "deb http://deb.debian.org/debian trixie-backports main" | sudo tee /etc/apt/sources.list.d/zdl-backports.list >/dev/null
        sudo apt-get -q update >/dev/null'
    # The versioned package the backports' meta package names, not the
    # meta package itself: the system's own stays as it is.
    test_kernel=$(vm "apt-cache -t trixie-backports depends linux-image-$flavour" \
        | sed -n 's/^ *Depends: \(linux-image-[0-9][^ ]*\)$/\1/p' | head -n 1)
    [[ -n $test_kernel ]] || fail "trixie-backports has no linux-image-$flavour"
fi
new=${test_kernel#linux-image-}
newer_than "$new" "$kept" || fail "$new is not newer than $kept"
echo "  installing $test_kernel without its headers (GRUB starts $kept, $running runs)"
# DKMS's hook may fail the kernel's package without headers, or only warn;
# GRUB's default must hold either way.
vm "sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y -t trixie-backports --no-install-recommends install $test_kernel \
    >/tmp/zdl-kernel.log 2>&1 || sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y --no-install-recommends install $test_kernel \
    >>/tmp/zdl-kernel.log 2>&1" || echo "  apt failed (as DKMS may make it): $(vm 'tail -n 3 /tmp/zdl-kernel.log')"
vm "test -d /lib/modules/$new" || fail "$test_kernel brought no modules: $(vm 'tail -n 20 /tmp/zdl-kernel.log')"
if vm "ls /lib/modules/$new/updates/dkms/zfs.ko* >/dev/null 2>&1"; then
    echo "  DKMS built ZFS for $new after all: its module is removed, as if the build had failed"
    vm "sudo rm -f /lib/modules/$new/updates/dkms/zfs.ko*"
    vm 'sudo /usr/libexec/zendiloos/zendiloos-agent choose-boot-kernel' >/dev/null || fail "choosing failed"
fi
# Whatever writes GRUB's configuration next lists the new kernel first;
# GRUB without zendiloOS's default would start it.
vm 'sudo update-grub >/dev/null 2>&1' || fail "update-grub failed"
vm "grep -m 1 -E '^[[:space:]]*linux[[:space:]]' /boot/grub/grub.cfg" | grep -q "vmlinuz-$new " \
    || fail "GRUB's first entry does not start $new: $(vm "grep -m 1 -E '^[[:space:]]*linux' /boot/grub/grub.cfg")"
[[ $(grub_default) == *"$(grub_entry_of "$kept")"* ]] || fail "GRUB's default left $kept: $(grub_default)"
check_now
overview=$(call GET /system/updates)
restart=false
[[ $kept != "$running" ]] && restart=true
jq -e --arg k "$kept" --arg n "$new" --argjson restart "$restart" '.boot.kernel == $k
    and .boot.reason == "newest_lacks_zfs" and .newest_kernel == $n and (.zfs_ready | not)
    and .restart_needed == $restart' <<<"$overview" >/dev/null \
    || fail "the overview: $(jq -c '{boot, newest_kernel, zfs_ready, restart_needed}' <<<"$overview")"
[[ $(grub_default) == *"$(grub_entry_of "$kept")"* ]] || fail "the check moved GRUB's default: $(grub_default)"
echo "  GRUB keeps starting $kept; $new waits for its ZFS module"

# A module there (a stand-in; the VM is not restarted): the new kernel
# starts. Gone again: back to the kept one, by a check.
vm "sudo install -d /lib/modules/$new/updates/dkms && sudo touch /lib/modules/$new/updates/dkms/zfs.ko.xz"
vm 'sudo /usr/libexec/zendiloos/zendiloos-agent choose-boot-kernel' >/dev/null || fail "choosing failed"
[[ $(grub_default) == *"$(grub_entry_of "$new")"* ]] || fail "GRUB's default is not $new with its module: $(grub_default)"
vm "sudo rm -f /lib/modules/$new/updates/dkms/zfs.ko.xz"
check_now
[[ $(grub_default) == *"$(grub_entry_of "$kept")"* ]] || fail "a check left GRUB's default on $new: $(grub_default)"

# The kernel removed: its hook chooses again, the default stays.
purge_kernel "$test_kernel" || fail "$test_kernel cannot be purged"
left=$(vm "dpkg-query -W -f='\${Package} \${db:Status-Abbrev}\n'" | awk -v base="-${new%%-*}" '$2 ~ /^i/ && index($1, base) {print $1}')
[[ -z $left ]] || fail "packages of $new stay: $left"
test_kernel=""
[[ $(grub_default) == *"$(grub_entry_of "$kept")"* ]] || fail "GRUB's default after the removal: $(grub_default)"
if vm "grep -q 'vmlinuz-$new ' /boot/grub/grub.cfg"; then fail "GRUB still lists $new"; fi

say "All update tests passed (put the VM back with: ZENDILOOS_VM=$ZENDILOOS_VM scripts/vm.sh install $current)"
