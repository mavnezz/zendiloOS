#!/usr/bin/env bash
# Installs zendiloOS from the installation ISO into a fresh QEMU machine, no
# one at the keyboard, and checks the result:
#
#   1. the installer runs from the ISO with the ISO's answers; what a person
#      would answer (the disk, root's password) comes as boot parameters
#   2. the installed system starts, zendiloOS answers over HTTPS, and its
#      setup is open: no code, on the console nor asked for
#   3. the first administrator is set up without a code (from the host,
#      which QEMU's NAT makes 10.0.2.2, an address of the own network), and
#      a ZFS pool is made on the second disk: ZFS was built for the
#      installed kernel
#
# Usage: scripts/iso-test.sh [ISO]   (the newest dist/zendiloos-*.iso)
#   ZENDILOOS_ISO_TEST_REUSE=1  skips the installation and starts the
#   machine installed by the last run again
#
# Needs KVM and the network (the packages come from Debian's mirror); takes
# 15 to 30 minutes. The machine lives in dev/iso-test and only listens on
# 127.0.0.1 (HTTPS on ZENDILOOS_ISO_TEST_PORT, 8450).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
iso=${1:-$(ls -t "$root"/dist/zendiloos-*-netinst.iso 2>/dev/null | head -1)}
[[ -f $iso ]] || { echo "no ISO; run 'just iso'" >&2; exit 1; }
dir=$root/dev/iso-test
port=${ZENDILOOS_ISO_TEST_PORT:-8450}
base="https://127.0.0.1:$port"
api="$base/api/v1"
password="correct horse battery"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }

wait_for() {
    local what=$1 check=$2 seconds=${3:-120}
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# Disks with serial numbers, as real ones have: /dev/disk/by-id names them.
machine=(qemu-system-x86_64 -enable-kvm -cpu host -smp 4 -m 4096 -display none
         -drive "file=$dir/system.qcow2,if=none,id=system" -device virtio-blk-pci,drive=system,serial=ZDLSYSTEM
         -drive "file=$dir/data.qcow2,if=none,id=data" -device virtio-blk-pci,drive=data,serial=ZDLDATA1)

stop() {
    [[ -f $dir/qemu.pid ]] && kill "$(cat "$dir/qemu.pid")" 2>/dev/null || true
    rm -f "$dir/qemu.pid"
}
trap stop EXIT

stop
if [[ ${ZENDILOOS_ISO_TEST_REUSE:-} == 1 && -f $dir/system.qcow2 ]]; then
    say "1. Reusing the machine installed before"
else
say "1. Installing from $(basename "$iso")"
rm -rf "$dir"
mkdir -p "$dir"
qemu-img create -q -f qcow2 "$dir/system.qcow2" 24G
qemu-img create -q -f qcow2 "$dir/data.qcow2" 8G
xorriso -osirrox on -indev "$iso" -extract /install.amd/vmlinuz "$dir/vmlinuz" \
    -extract /install.amd/initrd.gz "$dir/initrd.gz" 2>/dev/null
# What a person answers at the keyboard; the rest is the ISO's own. After
# `---`: kept for the installed system, whose console then goes to the
# serial port too.
answers=(
    preseed/file=/cdrom/zendiloos/preseed.cfg auto=true priority=critical
    locale=en_US.UTF-8 keyboard-configuration/xkb-keymap=us time/zone=UTC
    partman-auto/disk=/dev/vda partman-auto/method=regular partman-auto/choose_recipe=atomic
    partman-partitioning/confirm_write_new_label=true partman/choose_partition=finish
    partman/confirm=true partman/confirm_nooverwrite=true
    passwd/root-password=iso-test passwd/root-password-again=iso-test
    grub-installer/bootdev=/dev/vda
    console=ttyS0,115200n8 --- console=tty0 console=ttyS0,115200n8
)
# A monitor, to look at the installer's log console (VT4) should it stop.
timeout 3600 "${machine[@]}" -no-reboot -cdrom "$iso" -kernel "$dir/vmlinuz" -initrd "$dir/initrd.gz" \
    -append "${answers[*]}" -nic user,model=virtio-net-pci -serial "file:$dir/install.log" \
    -monitor "unix:$dir/monitor.sock,server,nowait" \
    || fail "the installer did not finish (see $dir/install.log)"
grep -q "Installation complete\|Requesting system reboot" "$dir/install.log" \
    || fail "the installation did not complete (see $dir/install.log)"
fi

say "2. Starting the installed system"
"${machine[@]}" -daemonize -pidfile "$dir/qemu.pid" -serial "file:$dir/boot.log" \
    -nic "user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:$port-:443"
wait_for "zendiloOS over HTTPS" "curl -skf $api/health" 600
if curl -skf "$api/setup" | jq -e .required >/dev/null; then
    curl -skf "$api/setup" | jq -e '.code_required == false' >/dev/null || fail "the setup asks for a code"
    wait_for "the open setup on the console" "grep -q 'to create the first administrator' $dir/boot.log" 120
    ! grep -q 'Setup code for the first administrator' "$dir/boot.log" || fail "a setup code on the console"
    setup=true
else
    setup=false
fi

say "3. The first administrator, and a ZFS pool on the second disk"
work=$(mktemp -d)
if $setup; then
    curl -skf -H "Origin: $base" -H 'Content-Type: application/json' \
        -d "$(jq -n --arg password "$password" '{name: "admin", password: $password}')" \
        "$api/setup" >/dev/null || fail "setup"
fi
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg password "$password" '{name: "admin", password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in"
call() {
    local args=(-sk -b "$work/jar" -X "$1" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf")")
    [[ -n ${3:-} ]] && args+=(-H 'Content-Type: application/json' -d "$3")
    curl "${args[@]}" "$api$2"
}
info=$(call GET /system/info)
echo "  zendiloOS $(jq -r .version <<<"$info") on $(jq -r '.os // .os_name // "Debian"' <<<"$info")"
if ! call GET /storage/pools | jq -e '.[] | select(.name == "tank")' >/dev/null; then
disk=$(call GET /storage/disks | jq -r '.[] | select(.usage == "unused" and .id_link != null) | .name' | head -1)
[[ -n $disk ]] || fail "the second disk is not offered: $(call GET /storage/disks | jq -c '[.[] | {name, usage}]')"
plan=$(call POST /storage/pools/plan "{\"name\":\"tank\",\"layout\":\"single\",\"disks\":[\"$disk\"]}")
job=$(call POST /storage/pools "{\"token\":\"$(jq -er .token <<<"$plan")\",\"confirm\":\"tank\"}" | jq -er .id) \
    || fail "no pool: $plan"
call GET /storage/disks | jq -e 'all(.[]; .name | startswith("fd") | not)' >/dev/null \
    || fail "a floppy drive is offered as a disk"
wait_for "the pool" "[[ \$(call GET /jobs/$job | jq -r .state) != running ]]" 300
state=$(call GET "/jobs/$job" | jq -r '.state + " " + (.error_detail // "")')
[[ $state == succeeded* ]] || fail "the pool: $state"
fi
call GET /storage/pools | jq -e '.[] | select(.name == "tank" and .state == "online")' >/dev/null \
    || fail "the pool is not online: $(call GET /storage/pools | jq -c .)"
rm -rf "$work"

say "The ISO installs a working zendiloOS (the machine stays in $dir until the next run)"
