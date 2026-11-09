#!/usr/bin/env bash
# A Debian 13 test VM for zendiloOS, with spare disks for storage work.
#
#   scripts/vm.sh create    download and verify the cloud image, create the disks
#   scripts/vm.sh start     boot in the background (console in dev/vm/console.log)
#   scripts/vm.sh install [deb]  install a package; the newest dist/zendiloos_*.deb by default
#   scripts/vm.sh ssh [cmd] log in as the image's user `debian`
#   scripts/vm.sh removable-attach <image> <id> [ro|rw] [serial]  plug a raw disk image in as a removable
#                           drive, with the serial number ZDL-RM-<id> or the one given
#   scripts/vm.sh removable-detach <id>          pull it out again, unannounced
#   scripts/vm.sh stop      power off
#   scripts/vm.sh destroy   delete the VM's disks (keeps the downloaded image)
#
# SSH is forwarded to 127.0.0.1:2222, the web interface to
# https://127.0.0.1:8443. With ZENDILOOS_VM_LISTEN=0.0.0.0 at 'start', the
# web interface is reachable from the network too (SSH stays local).
#
# ZENDILOOS_VM=2 (3, …) works with another VM beside the first: its own
# directory dev/vm2, SSH on 2223 and the web interface on 8444, and so on.
#
# With ZENDILOOS_VM_BRIDGE=br0 at 'start', the VM also joins the LAN through
# that bridge with an address of its own, as a real NAS would: Windows and
# macOS clients find it and use SMB and NFS. Needs the bridge on the host and
# QEMU's bridge helper allowed to use it (/etc/qemu/bridge.conf).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
instance=${ZENDILOOS_VM:-1}
[[ $instance =~ ^[1-9]$ ]] || { echo "ZENDILOOS_VM must be 1 to 9" >&2; exit 1; }
# The first VM keeps the directory it always had; all share the image.
image_dir=$root/dev/vm
dir=$image_dir
[[ $instance == 1 ]] || dir=$root/dev/vm$instance
base_url=https://cloud.debian.org/images/cloud/trixie/latest
image=debian-13-genericcloud-amd64.qcow2
ssh_port=$((2221 + instance))
https_port=$((8442 + instance))
https_listen=${ZENDILOOS_VM_LISTEN:-127.0.0.1}
lan_bridge=${ZENDILOOS_VM_BRIDGE:-}
# Fixed, so the LAN's DHCP server hands the VM the same address each time.
lan_mac=52:54:00:5a:d1:0$((4 + instance))
sata_disks=4
# A throwaway VM: its host key changes with every 'create'.
common_opts=(-i "$dir/id_ed25519" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
             -o LogLevel=ERROR -o ConnectTimeout=5)
ssh_opts=("${common_opts[@]}" -p "$ssh_port")
scp_opts=("${common_opts[@]}" -P "$ssh_port")

say() { printf '\033[1m%s\033[0m\n' "$*"; }

running() { [[ -f $dir/qemu.pid ]] && kill -0 "$(cat "$dir/qemu.pid")" 2>/dev/null; }

download() {
    say "Downloading $image"
    curl -fL --progress-bar -o "$image_dir/$image.part" "$base_url/$image"
    curl -fsSL -o "$image_dir/SHA512SUMS" "$base_url/SHA512SUMS"
    local expected
    expected=$(awk -v name="$image" '$2 == name { print $1 }' "$image_dir/SHA512SUMS")
    [[ -n $expected ]] || { echo "no checksum for $image" >&2; exit 1; }
    echo "$expected  $image_dir/$image.part" | sha512sum -c --quiet -
    mv "$image_dir/$image.part" "$image_dir/$image"
}

cloud_init_seed() {
    local seed=$dir/seed
    mkdir -p "$seed"
    local hostname=zendiloos-vm
    [[ $instance == 1 ]] || hostname=zendiloos-vm$instance
    printf 'instance-id: zendiloos-vm-%s\nlocal-hostname: %s\n' "$instance" "$hostname" > "$seed/meta-data"
    cat > "$seed/user-data" <<USERDATA
#cloud-config
ssh_pwauth: false
ssh_authorized_keys:
  - $(cat "$dir/id_ed25519.pub")
# QEMU forwards DNS to the host's first resolver, which it cannot always
# reach; public resolvers next to it keep the test VM working.
write_files:
  - path: /etc/systemd/resolved.conf.d/zendiloos-vm.conf
    content: |
      [Resolve]
      DNS=1.1.1.1 9.9.9.9
runcmd:
  - systemctl restart systemd-resolved
USERDATA
    xorriso -as mkisofs -quiet -output "$dir/seed.iso" -volid cidata -joliet -rock \
        "$seed/user-data" "$seed/meta-data"
}

create() {
    mkdir -p "$dir" "$image_dir"
    [[ -f $image_dir/$image ]] || download
    [[ -f $dir/id_ed25519 ]] || ssh-keygen -q -t ed25519 -N '' -C zendiloos-vm -f "$dir/id_ed25519"
    [[ -f $dir/system.qcow2 ]] && { echo "VM exists; run 'destroy' first" >&2; exit 1; }
    say "Creating disks"
    qemu-img create -q -f qcow2 -F qcow2 -b "$image_dir/$image" "$dir/system.qcow2" 20G
    for i in $(seq 1 "$sata_disks"); do qemu-img create -q -f qcow2 "$dir/sata-$i.qcow2" 8G; done
    qemu-img create -q -f qcow2 "$dir/nvme-1.qcow2" 8G
    cloud_init_seed
    say "Created. Next: start"
}

start() {
    running && { echo "already running"; return; }
    local disks=()
    for i in $(seq 1 "$sata_disks"); do
        # q35's built-in AHCI controller; QEMU emulates SMART for these.
        disks+=(-drive "file=$dir/sata-$i.qcow2,if=none,id=sata$i,format=qcow2"
                -device "ide-hd,drive=sata$i,bus=ide.$((i - 1)),serial=ZDL-SATA-$i")
    done
    local lan=()
    [[ -n $lan_bridge ]] && lan=(-netdev "bridge,id=lan,br=$lan_bridge" -device "virtio-net-pci,netdev=lan,mac=$lan_mac")
    say "Starting (console: ${dir#"$root"/}/console.log)"
    qemu-system-x86_64 \
        -machine q35,accel=kvm -cpu host -smp 2 -m 2048 \
        -drive "file=$dir/system.qcow2,if=none,id=system,format=qcow2" \
        -device virtio-blk-pci,drive=system,bootindex=0 \
        -drive "file=$dir/seed.iso,if=virtio,format=raw,readonly=on" \
        "${disks[@]}" \
        -drive "file=$dir/nvme-1.qcow2,if=none,id=nvme1,format=qcow2" \
        -device "nvme,drive=nvme1,serial=ZDL-NVME-1" \
        -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$ssh_port-:22,hostfwd=tcp:$https_listen:$https_port-:443" \
        -device virtio-net-pci,netdev=net0 \
        "${lan[@]}" \
        -device virtio-scsi-pci,id=scsi \
        -monitor "unix:$dir/monitor.sock,server,nowait" \
        -display none -serial "file:$dir/console.log" \
        -daemonize -pidfile "$dir/qemu.pid"
    wait_for_ssh
    [[ -z $lan_bridge ]] || join_lan
}

# DHCP on the LAN card. The NAT card stays the default route, so the VM
# reaches the internet as before.
join_lan() {
    printf '%s\n' 'network:' '  version: 2' '  ethernets:' '    lan:' '      match:' \
        "        macaddress: \"$lan_mac\"" '      dhcp4: true' '      dhcp4-overrides:' '        route-metric: 200' \
        | ssh "${ssh_opts[@]}" debian@127.0.0.1 'sudo tee /etc/netplan/60-zendiloos-lan.yaml >/dev/null
            sudo chmod 600 /etc/netplan/60-zendiloos-lan.yaml && sudo netplan apply'
    local address=''
    for _ in $(seq 1 30); do
        address=$(ssh "${ssh_opts[@]}" debian@127.0.0.1 \
            "ip -o link | grep -i '$lan_mac' | cut -d: -f2 | xargs -r ip -4 -br addr show dev | awk '{print \$3}'")
        [[ -n $address ]] && { say "In the LAN as ${address%/*}: https://${address%/*}/"; return; }
        sleep 2
    done
    echo "the VM got no address in the LAN; see 'ip addr' in the VM" >&2
}

wait_for_ssh() {
    for _ in $(seq 1 60); do
        ssh "${ssh_opts[@]}" debian@127.0.0.1 true 2>/dev/null && { say "SSH is up"; return; }
        sleep 2
    done
    echo "SSH did not come up; see dev/vm/console.log" >&2
    exit 1
}

install_package() {
    local deb=${1:-}
    [[ -n $deb ]] || deb=$(ls -t "$root"/dist/zendiloos_*_amd64.deb 2>/dev/null | head -1)
    [[ -n $deb && -f $deb ]] || { echo "no package; run 'just deb'" >&2; exit 1; }
    say "Installing $(basename "$deb")"
    scp -q "${scp_opts[@]}" "$deb" debian@127.0.0.1:/tmp/
    # ZFS comes from contrib and DKMS builds it for each kernel: the
    # headers' metapackage, as the ISO installs it, so that a kernel a
    # Debian update brings gets its headers, and ZFS, too.
    ssh "${ssh_opts[@]}" debian@127.0.0.1 "set -e
        sudo sed -i 's/^Components: main\$/Components: main contrib/' /etc/apt/sources.list.d/debian.sources
        echo 'zfs-dkms zfs-dkms/note-incompatible-licenses note' | sudo debconf-set-selections
        sudo apt-get update -qq
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-headers-\$(uname -r) linux-headers-cloud-amd64
        # --reinstall: a build of the same version replaces the installed one.
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --reinstall /tmp/$(basename "$deb")"
    say "Installed: https://127.0.0.1:$https_port/"
}

# A command to QEMU's monitor; what it answers, without its prompts.
monitor() {
    [[ -S $dir/monitor.sock ]] || { echo "no monitor; restart the VM (stop, start)" >&2; exit 1; }
    printf '%s\n' "$1" | nc -U -q 1 "$dir/monitor.sock" | tr -d '\r' | grep -v '^(qemu)\|^QEMU ' || true
}

# A raw image as a removable disk, as if plugged in. The cloud image's
# kernel has no USB at all, so it comes over SCSI with the removable bit,
# which is what the kernel tells USB sticks by.
removable_attach() {
    local image=$1 id=$2 readonly=${3:-} serial=${4:-ZDL-RM-$2}
    [[ -f $image && $id =~ ^[a-z0-9]+$ && ( -z $readonly || $readonly == ro || $readonly == rw ) \
        && $serial =~ ^[A-Za-z0-9_-]+$ ]] \
        || { echo "usage: removable-attach <image> <id> [ro|rw] [serial]" >&2; exit 1; }
    [[ $readonly == rw ]] && readonly=
    local out
    out=$(monitor "drive_add 0 if=none,id=$id,file=$(realpath "$image"),format=raw${readonly:+,readonly=on}")
    [[ $out == *OK* ]] || { echo "drive_add: $out" >&2; exit 1; }
    out=$(monitor "device_add scsi-hd,bus=scsi.0,drive=$id,id=$id-stick,removable=on,serial=$serial")
    [[ -z $out ]] || { echo "device_add: $out" >&2; exit 1; }
}

# Pulled out without warning; the drive goes with the device.
removable_detach() {
    local out
    out=$(monitor "device_del $1-stick")
    [[ -z $out ]] || { echo "device_del: $out" >&2; exit 1; }
}

stop() {
    running || { echo "not running"; return; }
    ssh "${ssh_opts[@]}" debian@127.0.0.1 'sudo systemctl poweroff' 2>/dev/null || true
    for _ in $(seq 1 30); do running || { say "Stopped"; return; }; sleep 1; done
    kill "$(cat "$dir/qemu.pid")"
}

destroy() {
    running && stop
    rm -f "${dir:?}"/system.qcow2 "${dir:?}"/sata-*.qcow2 "${dir:?}"/nvme-*.qcow2 \
          "${dir:?}"/seed.iso "${dir:?}"/console.log "${dir:?}"/qemu.pid "${dir:?}"/monitor.sock
    say "Destroyed (the downloaded image is kept)"
}

case ${1:-} in
    create) create ;;
    start) start ;;
    install) install_package "${2:-}" ;;
    ssh) shift; exec ssh "${ssh_opts[@]}" debian@127.0.0.1 "$@" ;;
    removable-attach) removable_attach "${2:-}" "${3:-}" "${4:-}" "${5:-}" ;;
    removable-detach) removable_detach "${2:?usage: removable-detach <id>}" ;;
    stop) stop ;;
    destroy) destroy ;;
    *) sed -n '2,/^set /{/^set /d;p}' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
