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
#   4. systemd-networkd manages the network (the ISO handed it over from
#      ifupdown), so zendiloOS can change it, and names resolve through
#      systemd-resolved: apt finds Debian's mirror when looking for updates
#   5. a configuration backup downloads encrypted with age and is restored
#      on the same machine: zendiloosd starts again with it, the
#      administrator signs in as before
#   0. before all, the boot menus offer "Rescue a zendiloOS system" (both on
#      amd64, UEFI's on arm64)
#
# Usage: scripts/iso-test.sh [ISO]   (the newest dist/zendiloos-*.iso)
#   ZENDILOOS_ISO_TEST_REUSE=1  skips the installation and starts the
#   machine installed by the last run again
#
# The machine is of the ISO's architecture (its name says which). amd64: a
# PC (qemu-system-x86_64) with KVM, 15 to 30 minutes. arm64: QEMU's `virt`
# machine (qemu-system-aarch64, package qemu-system-arm) with the UEFI
# firmware AAVMF (qemu-efi-aarch64; another in ZENDILOOS_ISO_TEST_AAVMF,
# the directory of AAVMF_CODE.fd and AAVMF_VARS.fd), with KVM on an arm64
# host and emulated (TCG) on any other, which takes hours, ZFS's build by
# DKMS most of them; every wait is longer by ZENDILOOS_ISO_TEST_PATIENCE
# (4 without KVM). Needs the network (the packages come from Debian's
# mirror). The machine lives in dev/iso-test and only listens on 127.0.0.1
# (HTTPS on ZENDILOOS_ISO_TEST_PORT, 8450).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
iso=${1:-$(ls -t "$root"/dist/zendiloos-*-netinst.iso 2>/dev/null | head -1)}
[[ -f $iso ]] || { echo "no ISO; run 'just iso'" >&2; exit 1; }
[[ $(basename "$iso") =~ -(amd64|arm64)-netinst\.iso$ ]] || { echo "not an ISO for amd64 or arm64: $iso" >&2; exit 1; }
arch=${BASH_REMATCH[1]}
dir=$root/dev/iso-test
port=${ZENDILOOS_ISO_TEST_PORT:-8450}
base="https://127.0.0.1:$port"
api="$base/api/v1"
password="correct horse battery"

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAILED: %s\033[0m\n' "$*" >&2; exit 1; }

wait_for() {
    local what=$1 check=$2 seconds=$((${3:-120} * patience))
    for _ in $(seq 1 "$seconds"); do
        if eval "$check" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    fail "timed out waiting for $what"
}

# The machine of the ISO's architecture, its CD drive, where the
# installer's kernel lies on the ISO and the serial console's name.
case $arch in
    amd64)
        machine=(qemu-system-x86_64 -enable-kvm -cpu host)
        cdrom=(-cdrom "$iso")
        installer=/install.amd console=ttyS0 patience=${ZENDILOOS_ISO_TEST_PATIENCE:-1}
        ;;
    arm64)
        firmware=${ZENDILOOS_ISO_TEST_AAVMF:-/usr/share/AAVMF}
        [[ -f $firmware/AAVMF_CODE.fd ]] \
            || { echo "no UEFI firmware in $firmware: install qemu-efi-aarch64" >&2; exit 1; }
        # Its variables (the boot entry the installer writes) stay with the
        # machine, as on a board.
        machine=(qemu-system-aarch64 -machine virt
                 -drive "if=pflash,format=raw,readonly=on,file=$firmware/AAVMF_CODE.fd"
                 -drive "if=pflash,format=raw,file=$dir/efivars.fd")
        if [[ $(uname -m) == aarch64 && -w /dev/kvm ]]; then
            machine+=(-accel kvm -cpu host)
            patience=${ZENDILOOS_ISO_TEST_PATIENCE:-1}
        else
            # Emulated: what QEMU emulates fastest of a CPU with all it has.
            machine+=(-accel tcg -cpu max,pauth-impdef=on)
            patience=${ZENDILOOS_ISO_TEST_PATIENCE:-4}
        fi
        # No IDE on virt: the ISO in a SCSI CD drive.
        cdrom=(-device virtio-scsi-pci,id=scsi -drive "file=$iso,if=none,id=cd,media=cdrom,readonly=on"
               -device scsi-cd,drive=cd,bus=scsi.0)
        installer=/install.a64 console=ttyAMA0
        ;;
esac
# Disks with serial numbers, as real ones have: /dev/disk/by-id names them.
machine+=(-smp 4 -m 4096 -display none
          -drive "file=$dir/system.qcow2,if=none,id=system" -device virtio-blk-pci,drive=system,serial=ZDLSYSTEM
          -drive "file=$dir/data.qcow2,if=none,id=data" -device virtio-blk-pci,drive=data,serial=ZDLDATA1)

stop() {
    [[ -f $dir/qemu.pid ]] && kill "$(cat "$dir/qemu.pid")" 2>/dev/null || true
    rm -f "$dir/qemu.pid"
}
trap stop EXIT

stop
say "0. The boot menus offer the rescue mode"
menus=$(mktemp -d)
# arm64 starts through UEFI only: no BIOS menu on its ISO.
extract=(-extract /boot/grub/grub.cfg "$menus/grub.cfg")
[[ $arch == amd64 ]] && extract+=(-extract /isolinux/zendiloos.cfg "$menus/isolinux.cfg")
xorriso -osirrox on -indev "$iso" "${extract[@]}" 2>/dev/null
grep -q "menuentry --hotkey=r 'Rescue a zendiloOS system'" "$menus/grub.cfg" || fail "no rescue entry for UEFI"
if [[ $arch == amd64 ]]; then
    bios=$(grep -A3 '^label zendiloos-rescue' "$menus/isolinux.cfg" || true)
    grep -q 'rescue/enable=true' <<<"$bios" || fail "no rescue entry for BIOS"
fi
rm -rf "$menus"
if [[ ${ZENDILOOS_ISO_TEST_REUSE:-} == 1 && -f $dir/system.qcow2 ]]; then
    say "1. Reusing the machine installed before"
else
say "1. Installing from $(basename "$iso")"
rm -rf "$dir"
mkdir -p "$dir"
qemu-img create -q -f qcow2 "$dir/system.qcow2" 24G
qemu-img create -q -f qcow2 "$dir/data.qcow2" 8G
[[ $arch == arm64 ]] && cp "$firmware/AAVMF_VARS.fd" "$dir/efivars.fd"
xorriso -osirrox on -indev "$iso" -extract "$installer/vmlinuz" "$dir/vmlinuz" \
    -extract "$installer/initrd.gz" "$dir/initrd.gz" 2>/dev/null
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
    "console=$console,115200n8" --- console=tty0 "console=$console,115200n8"
)
# A monitor, to look at the installer's log console (VT4) should it stop.
started=$SECONDS
timeout $((3600 * patience)) "${machine[@]}" -no-reboot "${cdrom[@]}" -kernel "$dir/vmlinuz" -initrd "$dir/initrd.gz" \
    -append "${answers[*]}" -nic user,model=virtio-net-pci -serial "file:$dir/install.log" \
    -monitor "unix:$dir/monitor.sock,server,nowait" \
    || fail "the installer did not finish (see $dir/install.log)"
grep -q "Installation complete\|Requesting system reboot" "$dir/install.log" \
    || fail "the installation did not complete (see $dir/install.log)"
echo "  installed in $(((SECONDS - started) / 60)) minutes"
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

say "4. systemd-networkd manages the network, and names resolve"
setup=$(call GET /network/setup)
echo "  the network's manager: $(jq -r .manager <<<"$setup"), managed: $(jq -r .managed <<<"$setup")"
jq -e '.managed == true and .manager == "networkd"' <<<"$setup" >/dev/null \
    || fail "systemd-networkd does not manage the network: $setup"
network=$(call GET /network)
card=$(jq -r '.default_routes[0].interface // empty' <<<"$network")
[[ -n $card ]] || fail "no default route: $network"
echo "  $card: $(jq -r --arg card "$card" '[.interfaces[] | select(.name == $card) | .addresses[] | select(.scope == "global") | .address] | join(", ")' <<<"$network")," \
    "name servers $(jq -r '.name_servers | join(", ")' <<<"$network")"
jq -e '.name_servers | length > 0' <<<"$network" >/dev/null || fail "no name servers: $network"
status() {
    curl -sk -o /dev/null -w '%{http_code}' -b "$work/jar" -X "$1" -H "Origin: $base" \
        -H "X-CSRF-Token: $(cat "$work/csrf")" "$api$2"
}
# The daily check may be under way already, a while after zendiloosd started.
wait_for "a look for updates to start" "[[ \$(status POST /system/updates/check) == 202 ]]" 900
wait_for "the look for updates" "call GET /system/updates | jq -e '.phase == \"idle\" and .checked_at != null'" 900
updates=$(call GET /system/updates)
jq -e '.packages_error == null' <<<"$updates" >/dev/null \
    || fail "apt could not reach Debian's mirror (name resolution?): $(jq -r .packages_error <<<"$updates")"
echo "  apt reached Debian's mirror: $(jq '.packages | length' <<<"$updates") packages to update"

say "5. A configuration backup, downloaded and restored"
passphrase="iso test passphrase"
curl -skf -b "$work/jar" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf")" \
    -H 'Content-Type: application/json' -d "$(jq -n --arg p "$passphrase" '{passphrase: $p}')" \
    -o "$work/backup.tar.zst.age" "$api/configuration/backup/download" || fail "no backup"
head -c 22 "$work/backup.tar.zst.age" | grep -q 'age-encryption.org/v1' || fail "the backup is not age's"
echo "  $(du -h "$work/backup.tar.zst.age" | cut -f1) encrypted"
restored=$(curl -sk -b "$work/jar" -H "Origin: $base" -H "X-CSRF-Token: $(cat "$work/csrf")" \
    -H 'Content-Type: application/octet-stream' -H "X-Zendiloos-Passphrase: $(printf %s "$passphrase" | base64)" \
    --data-binary "@$work/backup.tar.zst.age" "$api/configuration/restore")
jq -e '.host_name' <<<"$restored" >/dev/null || fail "not restored: $restored"
echo "  restored, warnings: $(jq -c .warnings <<<"$restored")"
sleep 5
wait_for "zendiloOS after the restore" "curl -skf $api/health" 180
curl -sk -c "$work/jar" -H "Origin: $base" -H 'Content-Type: application/json' \
    -d "$(jq -n --arg password "$password" '{name: "admin", password: $password}')" \
    "$api/auth/login" | jq -er .csrf_token >"$work/csrf" || fail "cannot sign in after the restore"
call GET /storage/pools | jq -e '.[] | select(.name == "tank")' >/dev/null || fail "the pool is gone after the restore"
rm -rf "$work"

say "The ISO installs a working zendiloOS (the machine stays in $dir until the next run)"
