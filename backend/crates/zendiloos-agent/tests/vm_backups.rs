//! Snapshots, backups and restores of virtual machines, a TPM and Secure
//! Boot, and the host's USB and PCI devices, against a fake host: `zfs`
//! keeps each zvol as a file in a folder that stands for `/dev/zvol` (a
//! "GiB" of it a MiB, to stay quick) and its snapshots as copies, `virsh`
//! keeps the domains' states as files, and `systemd-run` runs this very
//! binary's unit commands (`back-up-vm`, `restore-vm`, `remove-vm-backup`)
//! as the units would.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{DevicePaths, Erasable, ErasePolicy, HostPaths, Operations, Paths, VmPaths};
use zendiloos_agent_proto::{
    BackupRoot, BackupStem, ErrorCode, MacAddress, PciAddress, PoolName, Request, Response, Role,
    ShareLocation, ShareName, SharePath, SnapshotFrequency, SnapshotLabel, SnapshotNote, VmAction,
    VmBackupFile, VmBackupPart, VmBackupSettings, VmBoot, VmConsistency, VmCpus, VmDevices, VmDisk,
    VmDiskBus, VmDiskSize, VmDiskSlot, VmFirmware, VmId, VmMemory, VmNetwork, VmNic, VmNicModel, VmNicSlot,
    VmOs, VmSpec, VmStorage, VmUsbDevice, VolumeNumber,
};

const ID: &str = "0a1b2c3d4e5f";
const COPY: &str = "0a1b2c3d4e60";
const UUID: &str = "8b45cec4-586f-4885-973a-9e24c63b82fe";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    operations: Operations,
}

impl Host {
    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.path("calls.log")).unwrap_or_default();
        log.lines().map(|line| line.replace(&self.root.display().to_string(), "")).collect()
    }

    async fn run(&self, request: Request) -> Response {
        self.operations.execute(&request).await
    }

    fn zvol(&self, id: &str, slot: u8) -> PathBuf {
        self.path(&format!("zvol/tank/zendiloos.vms/{id}-disk{slot}"))
    }

    fn set_state(&self, id: &str, state: &str) {
        fs::write(self.path(&format!("state/zendiloos-{id}")), format!("{state}\n")).unwrap();
    }

    fn state_of(&self, id: &str) -> String {
        fs::read_to_string(self.path(&format!("state/zendiloos-{id}"))).unwrap_or_default().trim().to_owned()
    }

    /// Whether libvirt starts the domain with the machine.
    fn autostarts(&self, id: &str) -> bool {
        self.path(&format!("state/autostart.zendiloos-{id}")).exists()
    }

    /// The agent again, as after a restart.
    fn restarted(&self) -> Operations {
        operations_at(&self.root)
    }

    /// Waits until a program was called with `what` among its arguments.
    async fn called(&self, what: &str) {
        for _ in 0..800 {
            if self.calls().iter().any(|call| call.contains(what)) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("never called with {what}: {:?}", self.calls())
    }
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let log = root.join("calls.log");
    fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// zvols are files below `zvol/`; snapshots copies below `snaps/`, named
/// with `+` for `/`, their properties beside them, the order they were
/// taken in `snaps/order`; a snapshot's device shows below `zvol/` while
/// its zvol has `snapdev=visible`. `busy` lists zvols that do not go, and
/// with `pool-gone` the pool tank is not imported.
const ZFS: &str = r#"
root={root}
eval last=\${$#}
flat() { echo "$1" | tr / +; }
missing() { echo "cannot open '$last': dataset does not exist" >&2; exit 1; }
# With pool-gone, tank is not imported: ZFS says its datasets do not exist.
if [ -f "$root/pool-gone" ]; then case "$*" in *tank*) missing ;; esac; fi
case "$*" in
  'list -H -o name tank'|'list -H -o name cold') exit 0 ;;
  'list -H -o name tank/zendiloos.vms'|'list -H -o name cold/zendiloos.vms') exit 0 ;;
  'list -H -o name -t snapshot '*) test -f "$root/snaps/$(flat "$last")" ;;
  'list -H -o name -s createtxg -t snapshot -d 1 '*)
      grep "^$last@" "$root/snaps/order" 2>/dev/null; exit 0 ;;
  'list -H -o name '*/zendiloos.vms/*) test -f "$root/zvol/$last" || missing ;;
  'list -H -p -r -t snapshot '*)
      while read -r snap; do [ -f "$root/snaps/$(flat "$snap")" ] || continue
        printf '%s\t1793000000\t4096\t8192\t%s\t%s\t%s\n' "$snap" "$(cat "$root/snaps/$(flat "$snap").schedule" 2>/dev/null || echo -)" \
          "$(cat "$root/snaps/$(flat "$snap").note" 2>/dev/null || echo -)" "$(cat "$root/snaps/$(flat "$snap").consistency")"
      done < "$root/snaps/order" 2>/dev/null; exit 0 ;;
  'get -H -p -o value volsize '*) if [ -f "$root/zvol/$last" ]; then stat -c %s "$root/zvol/$last"; else missing; fi ;;
  'create -s -V '*) mkdir -p "$(dirname "$root/zvol/$last")"
      # With slow-devices, the device shows a moment after the zvol is made, as udev makes it.
      if [ -f "$root/slow-devices" ]; then (sleep 1; truncate -s "${4%G}M" "$root/zvol/$last") </dev/null >/dev/null 2>&1 &
      else truncate -s "${4%G}M" "$root/zvol/$last"; fi ;;
  'set volsize='*) v=${2#volsize=}; case "$v" in *G) truncate -s "${v%G}M" "$root/zvol/$last" ;; *) truncate -s "$v" "$root/zvol/$last" ;; esac ;;
  'set snapdev=visible '*) for s in "$root"/snaps/$(flat "$last")@*; do case "$s" in *.schedule|*.note|*.consistency) continue ;; esac
        [ -f "$s" ] && cp "$s" "$root/zvol/$(basename "$s" | tr + /)"; done; exit 0 ;;
  'inherit snapdev '*) rm -f "$root/zvol/$last"@*; exit 0 ;;
  snapshot*)
      schedule=-; note=-; consistency=-
      while [ "$1" != "${1#-}" ] || [ "$1" = snapshot ]; do
        case "$1" in -o) case "$2" in zendiloos:schedule=*) schedule=${2#*=} ;; zendiloos:note=*) note=${2#*=} ;; zendiloos:consistency=*) consistency=${2#*=} ;; esac; shift 2 ;; *) shift ;; esac
      done
      for snap in "$@"; do f="$root/snaps/$(flat "$snap")"; cp "$root/zvol/${snap%@*}" "$f"
        echo "$schedule" > "$f.schedule"; echo "$note" > "$f.note"; echo "$consistency" > "$f.consistency"; echo "$snap" >> "$root/snaps/order"; done ;;
  'destroy -r '*) if grep -qx "$last" "$root/busy" 2>/dev/null; then echo "cannot destroy '$last': dataset is busy" >&2; exit 1; fi
      rm -f "$root/zvol/$last"; for s in "$root"/snaps/$(flat "$last")@*; do rm -f "$s"; done ;;
  'destroy '*@*) [ -f "$root/snaps/$(flat "$last")" ] || exit 1; rm -f "$root/snaps/$(flat "$last")"* "$root/zvol/$last"
      grep -v "^$last\$" "$root/snaps/order" > "$root/snaps/order.new"; mv "$root/snaps/order.new" "$root/snaps/order" ;;
  'destroy '*) rm -f "$root/zvol/$last" ;;
  'rollback '*) while [ -f "$root/hold-rollback" ]; do sleep 0.05; done
      cp "$root/snaps/$(flat "$last")" "$root/zvol/${last%@*}"
      if [ "$2" = -r ]; then after=$(sed -n "\|^$last\$|,\$p" "$root/snaps/order" | tail -n +2 | grep "^${last%@*}@")
        for s in $after; do rm -f "$root/snaps/$(flat "$s")"*; grep -v "^$s\$" "$root/snaps/order" > "$root/snaps/order.new"; mv "$root/snaps/order.new" "$root/snaps/order"; done; fi
      echo "zfs rolled back $last" >> "$root/calls.log" ;;
esac
"#;

/// Domains are files below `state/` holding what they do, and
/// `autostart.<domain>` while they start with the machine; the guest agent
/// answers if `state/agent` is there, and only after libvirt gave up on it
/// with `state/agent-slow`. `libvirt-down`, `lookup-fails` (a domain there
/// that virsh cannot get), `define-fails`,
/// `suspend-fails`, `resume-fails` (once) and `autostart-fails` make
/// libvirt fail as it says.
const VIRSH: &str = r#"
state={root}/state
shift 2
case "$1" in
  domstate) if [ -f "$state/libvirt-down" ]; then echo 'error: failed to connect to the hypervisor' >&2; exit 1; fi
      if [ -f "$state/$2" ] && [ ! -f "$state/lookup-fails" ]; then cat "$state/$2"
      else echo "error: failed to get domain '$2'" >&2; exit 1; fi ;;
  list) if [ -f "$state/libvirt-down" ]; then echo 'error: failed to connect to the hypervisor' >&2; exit 1; fi
      for f in "$state"/zendiloos-*; do case "$f" in *.defined) ;; *) [ -e "$f" ] && basename "$f" ;; esac; done; true ;;
  define) if [ -f "$state/define-fails" ]; then echo 'error: XML error: refused' >&2; exit 1; fi
          name=$(sed -n 's/.*<name>\(.*\)<\/name>.*/\1/p' "$3"); cp "$3" "$state/$name.defined"
          [ -f "$state/$name" ] || echo 'shut off' > "$state/$name" ;;
  domuuid) test -f "$state/$2" && echo {uuid} ;;
  start) echo running > "$state/${2#--reset-nvram}"; [ "$2" = --reset-nvram ] && echo running > "$state/$3"; true ;;
  domfsfreeze) if [ -f "$state/agent-slow" ]; then echo 'error: Guest agent is not responding: Guest agent not available for now' >&2; exit 1; fi
      test -f "$state/agent" || { echo 'error: Guest agent is not responding: QEMU guest agent is not connected' >&2; exit 1; } ;;
  suspend) test ! -f "$state/suspend-fails" ;;
  resume) if [ -f "$state/resume-fails" ]; then rm "$state/resume-fails"; exit 1; fi ;;
  autostart) if [ -f "$state/autostart-fails" ]; then echo 'error: cannot change autostart' >&2; exit 1; fi
      if [ "$2" = --disable ]; then rm -f "$state/autostart.$3"
        # Turned off only after a restore marked itself: a power cut between the two starts it at boot.
        for marker in {root}/domains/*.restoring; do if [ -e "$marker" ]; then touch "$state/disabled-after-marking"; fi; done
      else touch "$state/autostart.$2"; fi ;;
  dominfo) test -f "$state/$2" || { echo "error: failed to get domain '$2'" >&2; exit 1; }
      echo "Name:           $2"; echo "State:          $(cat "$state/$2")"
      if [ -f "$state/autostart.$2" ]; then echo 'Autostart:      enable'; else echo 'Autostart:      disable'; fi ;;
  dumpxml) eval last=\${$#}; cat "$state/$last.defined" ;;
  domfsthaw|attach-device|detach-device) exit 0 ;;
  undefine) eval last=\${$#}; rm -f "$state/$last" "$state/autostart.$last" ;;
esac
"#;

/// Units: what `systemd-run` is asked runs here, as the unit would; not
/// before `hold-unit` is gone, and ending only once `hold-unit-end` is.
const SYSTEMD_RUN: &str = r#"
while [ "${1#--}" != "$1" ]; do shift; done
while [ -f {root}/hold-unit ]; do sleep 0.05; done
"$@"
status=$?
while [ -f {root}/hold-unit-end ]; do sleep 0.05; done
exit $status
"#;

fn host() -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in [
        "units",
        "state",
        "snaps",
        "zvol",
        "domains",
        "volume1/backups",
        "nvram",
        "swtpm",
        "firmware",
        "iommu",
        "consoles",
    ] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    fs::write(
        root.join("units/volume1-\\x40vms.mount"),
        "[Mount]\nWhat=tank/zendiloos.vms\nWhere=/volume1/@vms\n",
    )
    .unwrap();
    fs::write(root.join("cpuinfo"), "processor : 0\nvendor_id : GenuineIntel\nflags : vmx\n").unwrap();
    fs::write(root.join("meminfo"), "MemTotal: 2027168 kB\n").unwrap();
    fs::write(root.join("cmdline"), "quiet\n").unwrap();
    // swtpm's account: this test's own, which it may give files to.
    let me = std::os::unix::fs::MetadataExt::uid(&fs::metadata(&root).unwrap());
    let group = std::os::unix::fs::MetadataExt::gid(&fs::metadata(&root).unwrap());
    fs::write(
        root.join("passwd"),
        format!("root:x:0:0::/root:/bin/sh\ntss:x:{me}:{group}::/var/lib/swtpm:/bin/false\n"),
    )
    .unwrap();
    let fill =
        |template: &str| template.replace("{root}", &root.display().to_string()).replace("{uuid}", UUID);
    script(&root, "zfs", &fill(ZFS));
    script(&root, "virsh", &fill(VIRSH));
    script(&root, "systemd-run", &fill(SYSTEMD_RUN));
    // With unit-stuck, a VM's unit stays active whatever stop says.
    script(
        &root,
        "systemctl",
        &fill(
            "if [ \"$1\" = show ]; then if [ -f {root}/unit-stuck ]; then echo active; else echo inactive; fi; fi; exit 0",
        ),
    );
    script(&root, "udevadm", "");
    script(&root, "busctl", "exit 1");
    fs::write(root.join("libvirtd"), "").unwrap();
    fs::write(root.join("qemu"), "").unwrap();
    let operations = operations_at(&root);
    Host { _dir: dir, root, operations }
}

/// The agent on the fake host at `root`: another one, as after a restart.
fn operations_at(root: &Path) -> Operations {
    let root = root.to_path_buf();
    let tools = Tools {
        zfs: root.join("zfs"),
        virsh: root.join("virsh"),
        systemd_run: root.join("systemd-run"),
        systemctl: root.join("systemctl"),
        udevadm: root.join("udevadm"),
        busctl: root.join("busctl"),
        libvirtd: root.join("libvirtd"),
        qemu: root.join("qemu"),
        swtpm: root.join("swtpm-binary"),
        // Missing until a test makes it.
        virt_fw_vars: root.join("virt-fw-vars"),
        ..Tools::default()
    };
    let paths = Paths {
        host: HostPaths {
            units: root.join("units"),
            volumes: root.clone(),
            agent: PathBuf::from(env!("CARGO_BIN_EXE_zendiloos-agent")),
            share_mounts: false,
            ..HostPaths::default()
        },
        vms: VmPaths {
            kvm: root.join("kvm"),
            cpuinfo: root.join("cpuinfo"),
            meminfo: root.join("meminfo"),
            devices: DevicePaths {
                usb: root.join("usb"),
                pci: root.join("pci"),
                iommu_groups: root.join("iommu"),
                cmdline: root.join("cmdline"),
                pci_ids: root.join("pci.ids"),
            },
            firmware: root.join("firmware"),
            own_firmware: root.join("etc-firmware"),
            variables: root.join("variables"),
            certificates: root.join("certs"),
            nvram: root.join("nvram"),
            swtpm: root.join("swtpm"),
            transient: root.join("transient"),
            passwd: root.join("passwd"),
            domains: root.join("domains"),
            consoles: root.join("consoles"),
            tmpfiles: root.join("tmpfiles.conf"),
            apparmor: root.join("apparmor"),
            zvols: root.join("zvol"),
            network: root.join("network"),
            internal: root.join("internal.nft"),
        },
        ..Paths::default()
    };
    Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap()
}

fn tank() -> VmStorage {
    VmStorage { pool: PoolName::parse("tank").unwrap(), volume: VolumeNumber::new(1).unwrap() }
}

fn slot(n: u8) -> VmDiskSlot {
    VmDiskSlot::new(n).unwrap()
}

fn disk(n: u8, gib: u32) -> VmDisk {
    VmDisk { slot: slot(n), size: VmDiskSize::new(gib).unwrap(), bus: VmDiskBus::Virtio, location: tank() }
}

fn devices() -> VmDevices {
    VmDevices {
        disks: vec![disk(0, 1), disk(1, 2)],
        nics: vec![VmNic {
            slot: VmNicSlot::new(0).unwrap(),
            model: VmNicModel::Virtio,
            network: VmNetwork::User,
            mac: MacAddress::parse("52:54:00:00:00:01").unwrap(),
        }],
        boot: vec![VmBoot::Disk(slot(0))],
        ..VmDevices::default()
    }
}

fn spec_of(id: &str, firmware: VmFirmware, devices: VmDevices) -> VmSpec {
    spec_with(id, firmware, devices, false)
}

fn spec_with(id: &str, firmware: VmFirmware, devices: VmDevices, autostart: bool) -> VmSpec {
    VmSpec::new(
        VmId::parse(id).unwrap(),
        VmOs::Windows,
        VmCpus::new(2).unwrap(),
        VmMemory::new(1024).unwrap(),
        firmware,
        devices,
        autostart,
        VolumeNumber::new(1).unwrap(),
    )
    .unwrap()
}

fn label(text: &str) -> SnapshotLabel {
    SnapshotLabel::parse(text).unwrap()
}

fn vm() -> VmId {
    VmId::parse(ID).unwrap()
}

fn share() -> BackupRoot {
    BackupRoot::Share {
        location: ShareLocation {
            volume: VolumeNumber::new(1).unwrap(),
            share: ShareName::parse("backups").unwrap(),
        },
    }
}

fn refused(response: &Response, says: &str) {
    match response {
        Response::Error { message, .. } => assert!(message.contains(says), "{message} does not say {says}"),
        other => panic!("not refused: {other:?}"),
    }
}

/// A VM with two disks, data written into them as a guest would.
async fn defined(host: &Host) {
    defined_with(host, devices(), false).await;
}

async fn defined_with(host: &Host, devices: VmDevices, autostart: bool) {
    let spec = spec_with(ID, VmFirmware::Uefi, devices, autostart);
    assert_eq!(host.run(Request::DefineVm { spec }).await, Response::Done);
    fs::write(host.zvol(ID, 0), [b"boot sector".as_slice(), &[0u8; 2000], b"data"].concat()).unwrap();
    fs::OpenOptions::new().write(true).open(host.zvol(ID, 0)).unwrap().set_len(1 << 20).unwrap();
    fs::write(host.zvol(ID, 1), vec![7u8; 2 << 20]).unwrap();
}

#[tokio::test]
async fn a_snapshot_of_all_disks_at_once_rolls_back_only_while_off() {
    let host = host();
    defined(&host).await;
    let first = label("zdl-2026-11-12_03.00.00");
    let note = SnapshotNote::parse("before the update").unwrap();
    let taken = host
        .run(Request::SnapshotVm {
            id: vm(),
            label: first.clone(),
            schedules: vec![SnapshotFrequency::Daily],
            note: Some(note),
        })
        .await;
    assert_eq!(taken, Response::VmSnapshotTaken { consistency: VmConsistency::Off });
    let snapshot = host.calls().into_iter().find(|call| call.starts_with("zfs snapshot")).unwrap();
    assert_eq!(
        snapshot,
        format!(
            "zfs snapshot -o zendiloos:consistency=off -o zendiloos:schedule=daily -o zendiloos:note=before the update \
             tank/zendiloos.vms/{ID}-disk0@zdl-2026-11-12_03.00.00 tank/zendiloos.vms/{ID}-disk1@zdl-2026-11-12_03.00.00"
        ),
        "one zfs snapshot of both disks"
    );
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(zfs_list.contains(&format!("tank/zendiloos.vms/{ID}-disk0@zdl-2026-11-12_03.00.00\t1793000000\t4096\t8192\tdaily\tbefore the update\toff")), "{zfs_list}");
    assert_eq!(
        host.run(Request::SnapshotVm { id: vm(), label: first.clone(), schedules: vec![], note: None })
            .await
            .error_code(),
        Some(ErrorCode::Conflict)
    );

    // The guest writes on; a second snapshot follows, then more writes.
    fs::write(host.zvol(ID, 0), vec![1u8; 1 << 20]).unwrap();
    let second = label("zdl-2026-11-12_04.00.00");
    host.run(Request::SnapshotVm { id: vm(), label: second.clone(), schedules: vec![], note: None }).await;
    fs::write(host.zvol(ID, 0), vec![2u8; 1 << 20]).unwrap();

    // Running: no rollback; then crash-consistent without the guest agent,
    // frozen with it.
    host.set_state(ID, "running");
    refused(
        &host.run(Request::RollbackVmSnapshot { id: vm(), label: first.clone(), newer: true }).await,
        "not shut off",
    );
    let crash = host
        .run(Request::SnapshotVm {
            id: vm(),
            label: label("zdl-2026-11-12_05.00.00"),
            schedules: vec![],
            note: None,
        })
        .await;
    assert_eq!(crash, Response::VmSnapshotTaken { consistency: VmConsistency::Crash });
    fs::write(host.path("state/agent"), "").unwrap();
    let frozen = host
        .run(Request::SnapshotVm {
            id: vm(),
            label: label("zdl-2026-11-12_06.00.00"),
            schedules: vec![],
            note: None,
        })
        .await;
    assert_eq!(frozen, Response::VmSnapshotTaken { consistency: VmConsistency::Frozen });
    let calls = host.calls();
    let freeze =
        calls.iter().rposition(|call| call.ends_with(&format!("domfsfreeze zendiloos-{ID}"))).unwrap();
    let thaw = calls.iter().rposition(|call| call.ends_with(&format!("domfsthaw zendiloos-{ID}"))).unwrap();
    let snap = calls.iter().rposition(|call| call.starts_with("zfs snapshot")).unwrap();
    assert!(freeze < snap && snap < thaw, "frozen around the snapshot: {calls:?}");
    for later in ["zdl-2026-11-12_05.00.00", "zdl-2026-11-12_06.00.00"] {
        assert_eq!(
            host.run(Request::DestroyVmSnapshot { id: vm(), label: label(later) }).await,
            Response::Done
        );
    }

    // Off: back to the first only once the newer ones may go.
    host.set_state(ID, "shut off");
    refused(
        &host.run(Request::RollbackVmSnapshot { id: vm(), label: first.clone(), newer: false }).await,
        "snapshots after",
    );
    assert_eq!(
        host.run(Request::RollbackVmSnapshot { id: vm(), label: second.clone(), newer: false }).await,
        Response::Done
    );
    assert_eq!(fs::read(host.zvol(ID, 0)).unwrap(), vec![1u8; 1 << 20], "the newest is no trouble");
    assert_eq!(
        host.run(Request::RollbackVmSnapshot { id: vm(), label: first.clone(), newer: true }).await,
        Response::Done
    );
    assert!(fs::read(host.zvol(ID, 0)).unwrap().starts_with(b"boot sector"));
    assert!(
        host.calls()
            .contains(&format!("zfs rollback -r tank/zendiloos.vms/{ID}-disk0@zdl-2026-11-12_03.00.00"))
    );
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(!zfs_list.contains("zdl-2026-11-12_04.00.00"), "the newer one went: {zfs_list}");
    assert_eq!(host.run(Request::DestroyVmSnapshot { id: vm(), label: first.clone() }).await, Response::Done);
    refused(&host.run(Request::DestroyVmSnapshot { id: vm(), label: first }).await, "no snapshot");
}

trait Code {
    fn error_code(&self) -> Option<ErrorCode>;
}

impl Code for Response {
    fn error_code(&self) -> Option<ErrorCode> {
        match self {
            Response::Error { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// zendiloOS's settings, as zendiloosd sends them.
fn settings() -> VmBackupSettings {
    VmBackupSettings::parse(r#"{"name":"office","cpus":2}"#).unwrap()
}

async fn backed_up(host: &Host) -> Response {
    backed_up_as(host, "20261112-030000").await
}

async fn backed_up_as(host: &Host, stem: &str) -> Response {
    backed_up_with(host, stem, vec![slot(0), slot(1)]).await
}

/// A backup of the VM, recorded with the disks in `disks`.
async fn backed_up_with(host: &Host, stem: &str, disks: Vec<VmDiskSlot>) -> Response {
    host.run(Request::BackUpVm {
        id: vm(),
        root: share(),
        folder: SharePath::parse("vms").unwrap(),
        stem: BackupStem::parse(stem).unwrap(),
        label: label("zdl-2026-11-12_03.00.00"),
        settings: settings(),
        disks,
    })
    .await
}

/// A restore of the backup in `folder` into the VM `spec` describes, over
/// it or as a new one.
fn restore_request(
    spec: VmSpec,
    folder: &SharePath,
    files: &[VmBackupFile],
    over: bool,
    remove: Vec<VmDiskSlot>,
) -> Request {
    Request::RestoreVm {
        spec,
        root: share(),
        folder: folder.clone(),
        files: files.to_vec(),
        over,
        remove,
        label: label("zdl-2026-11-12_09.00.00"),
    }
}

/// A backup of the VM: its folder and files.
async fn a_backup(host: &Host) -> (SharePath, Vec<VmBackupFile>) {
    match backed_up(host).await {
        Response::VmBackedUp { folder, files, .. } => (folder, files),
        other => panic!("not backed up: {other:?}"),
    }
}

#[tokio::test]
async fn a_backup_reads_the_disks_from_a_snapshot_and_restores_as_a_new_vm_or_over_the_old() {
    let host = host();
    defined(&host).await;
    fs::write(host.path(&format!("nvram/zendiloos-{ID}_VARS.fd")), b"uefi variables").unwrap();
    fs::create_dir_all(host.path(&format!("swtpm/{UUID}/tpm2"))).unwrap();
    fs::write(host.path(&format!("swtpm/{UUID}/tpm2/tpm2-00.permall")), b"tpm state").unwrap();
    let disk0 = fs::read(host.zvol(ID, 0)).unwrap();
    let disk1 = fs::read(host.zvol(ID, 1)).unwrap();

    let Response::VmBackedUp { folder, files, manifest, consistency } = backed_up(&host).await else {
        panic!("not backed up: {:?}", host.calls())
    };
    assert_eq!(folder.as_str(), format!("vms/{ID}/20261112-030000"));
    assert_eq!(consistency, VmConsistency::Off);
    let parts: Vec<VmBackupPart> = files.iter().map(|file| file.part).collect();
    assert_eq!(
        parts,
        [
            VmBackupPart::Disk { slot: slot(0), volsize: 1 << 20 },
            VmBackupPart::Disk { slot: slot(1), volsize: 2 << 20 },
            VmBackupPart::Nvram,
            VmBackupPart::Tpm
        ]
    );
    let place = host.path(&format!("volume1/backups/{}", folder.as_str()));
    assert_eq!(zstd::decode_all(&fs::read(place.join("disk0.raw.zst")).unwrap()[..]).unwrap(), disk0);
    assert_eq!(fs::read(place.join("tpm2-00.permall")).unwrap(), b"tpm state");
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(place.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(written["settings"]["name"], "office");
    assert_eq!(manifest.path.as_str(), format!("{}/manifest.json", folder.as_str()));
    let calls = host.calls();
    let visible = calls
        .iter()
        .position(|c| c == &format!("zfs set snapdev=visible tank/zendiloos.vms/{ID}-disk0"))
        .unwrap();
    let unit = calls.iter().position(|c| c.starts_with("systemd-run")).unwrap();
    let hidden = calls
        .iter()
        .position(|c| c == &format!("zfs inherit snapdev tank/zendiloos.vms/{ID}-disk0"))
        .unwrap();
    assert!(visible < unit && unit < hidden, "{calls:?}");
    let unit = &calls[unit];
    for property in [
        "--unit=zendiloos-vm-backup-0a1b2c3d4e5f.service",
        "DeviceAllow=block-zvol r",
        "PrivateDevices=no",
        "ReadWritePaths=-/volume1/backups",
        " back-up-vm ",
    ] {
        assert!(unit.contains(property), "{property} not in {unit}");
    }
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(zfs_list.is_empty(), "the backup's snapshot went: {zfs_list}");
    refused(&backed_up(&host).await, "exists already");

    // As a new VM: its disks made, written as they were, the variables and
    // the TPM's state in place.
    let copy = spec_of(COPY, VmFirmware::Uefi, VmDevices { tpm: true, ..devices() });
    fs::write(host.path("swtpm-binary"), "").unwrap();
    let restore = |spec: VmSpec, over: bool, files: Vec<_>, remove: Vec<VmDiskSlot>| Request::RestoreVm {
        spec,
        root: share(),
        folder: folder.clone(),
        files,
        over,
        remove,
        label: label("zdl-2026-11-12_09.00.00"),
    };
    assert_eq!(host.run(restore(copy.clone(), false, files.clone(), vec![])).await, Response::Done);
    assert_eq!(fs::read(host.zvol(COPY, 0)).unwrap(), disk0);
    assert_eq!(fs::read(host.zvol(COPY, 1)).unwrap(), disk1);
    assert_eq!(fs::read(host.path(&format!("nvram/zendiloos-{COPY}_VARS.fd"))).unwrap(), b"uefi variables");
    let state = host.path(&format!("swtpm/{UUID}/tpm2/tpm2-00.permall"));
    assert_eq!(fs::read(&state).unwrap(), b"tpm state");
    assert_eq!(fs::metadata(&state).unwrap().permissions().mode() & 0o777, 0o600, "swtpm's alone");
    let unit =
        host.calls().into_iter().find(|c| c.starts_with("systemd-run") && c.contains("restore-vm")).unwrap();
    assert!(unit.contains("DeviceAllow=block-zvol rw") && unit.contains("ReadWritePaths=-/nvram"), "{unit}");
    assert_eq!(
        host.run(restore(copy, false, files.clone(), vec![])).await.error_code(),
        Some(ErrorCode::Conflict)
    );

    // Over the old one, off: back as it was; a third disk it has now goes.
    fs::write(host.zvol(ID, 0), vec![9u8; 1 << 20]).unwrap();
    let three = VmDevices { disks: vec![disk(0, 1), disk(1, 2), disk(2, 1)], ..devices() };
    host.run(Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, three), remove: vec![] }).await;
    assert!(host.zvol(ID, 2).exists());
    host.set_state(ID, "running");
    refused(
        &host
            .run(restore(spec_of(ID, VmFirmware::Uefi, devices()), true, files.clone(), vec![slot(2)]))
            .await,
        "not shut off",
    );
    host.set_state(ID, "shut off");
    assert_eq!(
        host.run(restore(spec_of(ID, VmFirmware::Uefi, devices()), true, files.clone(), vec![slot(2)])).await,
        Response::Done
    );
    assert_eq!(fs::read(host.zvol(ID, 0)).unwrap(), disk0);
    assert!(!host.zvol(ID, 2).exists(), "the disk the backup did not have went");
    assert!(
        host.calls().iter().any(|c| c.starts_with("zfs snapshot") && c.contains("zdl-2026-11-12_09.00.00"))
    );
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(!zfs_list.contains("zdl-2026-11-12_09.00.00"), "the snapshot from before went once restored");

    // A file not the one recorded: the restore fails and the disks are as
    // they were before it.
    fs::write(host.zvol(ID, 0), vec![5u8; 1 << 20]).unwrap();
    let image = place.join("disk0.raw.zst");
    let other = zstd::encode_all(&vec![6u8; 1 << 20][..], 3).unwrap();
    let mut changed = files.clone();
    changed[0].bytes = other.len() as u64;
    fs::remove_file(&image).unwrap();
    fs::write(&image, &other).unwrap();
    fs::set_permissions(&image, fs::Permissions::from_mode(0o644)).unwrap();
    refused(
        &host.run(restore(spec_of(ID, VmFirmware::Uefi, devices()), true, changed, vec![])).await,
        "SHA-256 differs",
    );
    assert_eq!(fs::read(host.zvol(ID, 0)).unwrap(), vec![5u8; 1 << 20], "back to what it was");

    // The backup goes, its folders with it.
    let parts = files.iter().map(|file| file.part).collect();
    assert_eq!(
        host.run(Request::RemoveVmBackup { root: share(), folder: folder.clone(), files: parts }).await,
        Response::Done
    );
    assert!(!place.exists() && host.path("volume1/backups/vms").exists());
    assert_eq!(host.run(Request::EndVmWork { id: vm() }).await, Response::VmWorkEnded { stopped: false });
}

#[tokio::test]
async fn a_tpm_and_secure_boot_need_their_packages_and_switching_firmware_starts_its_variables_afresh() {
    let host = host();
    let secure = VmDevices { tpm: true, secure_boot: true, ..devices() };
    refused(
        &host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, secure.clone()) }).await,
        "swtpm is not installed",
    );
    fs::write(host.path("swtpm-binary"), "").unwrap();
    refused(
        &host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, secure.clone()) }).await,
        "Microsoft's keys",
    );
    let Response::VmHost { swtpm, secure_boot, .. } = host.run(Request::VmHost {}).await else {
        panic!("no facts")
    };
    assert!(swtpm && !secure_boot);
    fs::write(
        host.path("firmware/40-edk2-x86_64-secure-enrolled.json"),
        r#"{"mapping":{"executable":{"filename":"/usr/share/OVMF/OVMF_CODE_4M.ms.fd"},"nvram-template":{"filename":"/usr/share/OVMF/OVMF_VARS_4M.ms.fd"}},"features":["acpi-s3","enrolled-keys","requires-smm","secure-boot"]}"#,
    )
    .unwrap();
    let Response::VmHost { secure_boot, .. } = host.run(Request::VmHost {}).await else { panic!("no facts") };
    assert!(secure_boot);
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, secure.clone()) }).await,
        Response::Done
    );
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(
        xml.contains("<tpm model='tpm-crb'>")
            && xml.contains("name='enrolled-keys'")
            && xml.contains("<smm state='on'/>")
    );
    // A TPM more is no other firmware; Secure Boot off is.
    let start = || Request::ControlVm { id: vm(), action: VmAction::Start };
    assert_eq!(host.run(start()).await, Response::Done);
    assert!(host.calls().contains(&format!("virsh --connect qemu:///system start zendiloos-{ID}")));
    host.set_state(ID, "shut off");
    let plain = VmDevices { tpm: true, ..devices() };
    assert_eq!(
        host.run(Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, plain), remove: vec![] }).await,
        Response::Done
    );
    assert!(host.path(&format!("domains/zendiloos-{ID}.reset-nvram")).exists());
    assert_eq!(host.run(start()).await, Response::Done);
    assert!(
        host.calls().contains(&format!("virsh --connect qemu:///system start --reset-nvram zendiloos-{ID}"))
    );
    assert!(!host.path(&format!("domains/zendiloos-{ID}.reset-nvram")).exists(), "once");
}

#[tokio::test]
async fn with_virt_firmware_a_vm_with_secure_boot_is_defined_with_microsofts_certificates_of_2023() {
    let host = host();
    fs::write(host.path("swtpm-binary"), "").unwrap();
    let template = host.path("OVMF_VARS_4M.ms.fd");
    fs::write(&template, "Microsoft Corporation KEK CA 2011").unwrap();
    let description = serde_json::json!({
        "description": "UEFI firmware for x86_64, with Secure Boot, required SMM, SB enabled, MS certs enrolled",
        "mapping": {
            "device": "flash",
            "executable": {"filename": "/usr/share/OVMF/OVMF_CODE_4M.ms.fd", "format": "raw"},
            "nvram-template": {"filename": template, "format": "raw"},
        },
        "targets": [{"architecture": "x86_64", "machines": ["pc-q35-*"]}],
        "features": ["acpi-s3", "enrolled-keys", "requires-smm", "secure-boot"],
    });
    fs::write(host.path("firmware/40-edk2-x86_64-secure-enrolled.json"), description.to_string()).unwrap();
    let Response::VmHost { secure_boot, secure_boot_2023, .. } = host.run(Request::VmHost {}).await else {
        panic!("no facts")
    };
    assert!(secure_boot && !secure_boot_2023, "without virt-firmware, 2011's only");

    fs::create_dir_all(host.path("certs")).unwrap();
    for name in ["MicrosoftCorporationKEK2KCA2023.pem", "WindowsUEFICA2023.pem", "MicrosoftUEFICA2023.pem"] {
        fs::write(host.path(&format!("certs/{name}")), name).unwrap();
    }
    // --input <from> --output <to>, then what it adds.
    script(
        &host.path(""),
        "virt-fw-vars",
        "cp \"$2\" \"$4\"\nprintf 'Microsoft Corporation KEK 2K CA 2023 Windows UEFI CA 2023 Microsoft UEFI CA 2023' >> \"$4\"",
    );
    let Response::VmHost { secure_boot_2023, .. } = host.run(Request::VmHost {}).await else {
        panic!("no facts")
    };
    assert!(secure_boot_2023);

    let secure = VmDevices { tpm: true, secure_boot: true, ..devices() };
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, secure) }).await,
        Response::Done
    );
    let calls = host.calls();
    let made = calls.iter().position(|call| call.starts_with("virt-fw-vars --input")).expect("made");
    let defined = calls.iter().position(|call| call.contains(" define ")).expect("defined");
    assert!(made < defined, "libvirt picks the firmware as it defines the domain");
    let ours: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(host.path("etc-firmware/30-zendiloos-x86_64-secure-enrolled.json")).unwrap(),
    )
    .unwrap();
    let made = host.path("variables/OVMF_VARS_4M.ms.fd");
    assert_eq!(ours["mapping"]["nvram-template"]["filename"], made.to_str().unwrap());
    assert!(fs::read_to_string(&made).unwrap().contains("Windows UEFI CA 2023"));
}

#[tokio::test]
async fn usb_devices_plug_in_while_it_runs_and_a_missing_one_keeps_it_from_starting() {
    let host = host();
    let keyboard = VmUsbDevice { vendor: 0x046d, product: 0xc31c };
    let with = VmDevices { usb: vec![keyboard], ..devices() };
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, with.clone()) }).await,
        Response::Done
    );
    refused(
        &host.run(Request::ControlVm { id: vm(), action: VmAction::Start }).await,
        "046d:c31c the virtual machine is given is not plugged in",
    );
    let plug = |name: &str| {
        let place = host.path(&format!("usb/{name}"));
        fs::create_dir_all(&place).unwrap();
        for (file, value) in [("idVendor", "046d"), ("idProduct", "c31c"), ("bDeviceClass", "00")] {
            fs::write(place.join(file), format!("{value}\n")).unwrap();
        }
    };
    plug("1-2");
    plug("1-3");
    refused(
        &host.run(Request::ControlVm { id: vm(), action: VmAction::Start }).await,
        "more than one USB device 046d:c31c",
    );
    fs::remove_dir_all(host.path("usb/1-3")).unwrap();
    assert_eq!(host.run(Request::ControlVm { id: vm(), action: VmAction::Start }).await, Response::Done);
    let Response::VmHostDevices { usb, iommu, .. } = host.run(Request::VmHostDevices {}).await else {
        panic!("no devices")
    };
    assert_eq!((usb.len(), usb[0].port.as_str()), (1, "1-2"));
    assert!(!iommu.on && iommu.cpu_vendor.as_deref() == Some("GenuineIntel"));
    // Pulled out of the running VM, and plugged in again.
    let without = spec_of(ID, VmFirmware::Uefi, devices());
    let change = host
        .run(Request::ChangeVmUsb { spec: without.clone(), attach: vec![], detach: vec![keyboard] })
        .await;
    assert_eq!(change, Response::Done);
    assert!(
        host.calls()
            .iter()
            .any(|c| c.contains(&format!("detach-device zendiloos-{ID}")) && c.ends_with("--live"))
    );
    let again = host
        .run(Request::ChangeVmUsb {
            spec: spec_of(ID, VmFirmware::Uefi, with),
            attach: vec![keyboard],
            detach: vec![],
        })
        .await;
    assert_eq!(again, Response::Done);
    assert!(host.calls().iter().any(|c| c.contains(&format!("attach-device zendiloos-{ID}"))));
    refused(
        &host.run(Request::ChangeVmUsb { spec: without, attach: vec![keyboard], detach: vec![] }).await,
        "not those the VM is given",
    );
    // PCI devices only with an IOMMU.
    let card = VmDevices { pci: vec![PciAddress::parse("0000:01:00.0").unwrap()], ..devices() };
    refused(
        &host.run(Request::DefineVm { spec: spec_of(COPY, VmFirmware::Uefi, card) }).await,
        "IOMMU is off",
    );
}

#[tokio::test]
async fn a_running_vm_on_two_pools_is_paused_for_its_snapshot_without_a_guest_agent() {
    let host = host();
    fs::write(
        host.path("units/volume2-\\x40vms.mount"),
        "[Mount]\nWhat=cold/zendiloos.vms\nWhere=/volume2/@vms\n",
    )
    .unwrap();
    let cold = VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() };
    let two = VmDevices { disks: vec![disk(0, 1), VmDisk { location: cold, ..disk(1, 1) }], ..devices() };
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, two) }).await,
        Response::Done
    );
    host.set_state(ID, "running");
    let taken = host
        .run(Request::SnapshotVm {
            id: vm(),
            label: label("zdl-2026-11-12_07.00.00"),
            schedules: vec![],
            note: None,
        })
        .await;
    assert_eq!(taken, Response::VmSnapshotTaken { consistency: VmConsistency::Crash });
    let calls = host.calls();
    let at = |end: &str| {
        calls.iter().position(|call| call.ends_with(end)).unwrap_or_else(|| panic!("{end}: {calls:?}"))
    };
    let snapshots: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.starts_with("zfs snapshot"))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(snapshots.len(), 2, "one per pool");
    assert!(
        at(&format!("suspend zendiloos-{ID}")) < snapshots[0]
            && snapshots[1] < at(&format!("resume zendiloos-{ID}"))
    );
    // Paused already: as at a power cut, and not paused again.
    host.set_state(ID, "paused");
    let paused = host
        .run(Request::SnapshotVm {
            id: vm(),
            label: label("zdl-2026-11-12_08.00.00"),
            schedules: vec![],
            note: None,
        })
        .await;
    assert_eq!(paused, Response::VmSnapshotTaken { consistency: VmConsistency::Crash });
    assert_eq!(host.calls().iter().filter(|call| call.contains("suspend")).count(), 1);
}

/// What a disk of the VM holds now.
fn holds(host: &Host, slot: u8) -> Vec<u8> {
    fs::read(host.zvol(ID, slot)).unwrap()
}

#[tokio::test]
async fn while_a_restore_writes_a_vms_disks_it_is_not_started_and_other_vms_go_on() {
    let host = host();
    defined(&host).await;
    let (folder, files) = a_backup(&host).await;
    let other = VmId::parse(COPY).unwrap();
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(COPY, VmFirmware::Uefi, devices()) }).await,
        Response::Done
    );
    fs::write(host.path("hold-unit"), "").unwrap();
    let spec = spec_of(ID, VmFirmware::Uefi, devices());
    let restore = host.run(restore_request(spec.clone(), &folder, &files, true, vec![]));
    let meanwhile = async {
        host.called("restore-vm").await;
        let quickly = std::time::Duration::from_secs(5);
        let snapshot = Request::SnapshotVm {
            id: other,
            label: label("zdl-2026-11-12_10.00.00"),
            schedules: vec![],
            note: None,
        };
        let snapshot = tokio::time::timeout(quickly, host.run(snapshot)).await;
        let start = Request::ControlVm { id: vm(), action: VmAction::Start };
        let start = tokio::time::timeout(quickly, host.run(start)).await;
        let change = Request::UpdateVm { spec, remove: vec![] };
        let change = tokio::time::timeout(quickly, host.run(change)).await;
        let state = host.state_of(ID);
        fs::remove_file(host.path("hold-unit")).unwrap();
        (snapshot, start, change, state)
    };
    let (restored, (snapshot, start, change, state)) = tokio::join!(restore, meanwhile);
    assert_eq!(
        snapshot.expect("another VM is snapshotted while the restore writes"),
        Response::VmSnapshotTaken { consistency: VmConsistency::Off }
    );
    for answer in [start.expect("refused at once"), change.expect("refused at once")] {
        assert_eq!(answer.error_code(), Some(ErrorCode::Conflict), "{answer:?}");
        refused(&answer, "restore");
    }
    assert_eq!(state, "shut off");
    assert_eq!(restored, Response::Done);
    assert!(!host.calls().iter().any(|call| call.ends_with(&format!(" start zendiloos-{ID}"))));
}

#[tokio::test]
async fn a_start_waits_for_a_rollback_of_the_vms_disks() {
    let host = host();
    defined(&host).await;
    let first = label("zdl-2026-11-12_03.00.00");
    let taken =
        host.run(Request::SnapshotVm { id: vm(), label: first.clone(), schedules: vec![], note: None }).await;
    assert_eq!(taken, Response::VmSnapshotTaken { consistency: VmConsistency::Off });
    fs::write(host.zvol(ID, 0), vec![3u8; 1 << 20]).unwrap();
    fs::write(host.path("hold-rollback"), "").unwrap();
    let rollback = host.run(Request::RollbackVmSnapshot { id: vm(), label: first, newer: false });
    let start = async {
        host.called("zfs rollback").await;
        host.run(Request::ControlVm { id: vm(), action: VmAction::Start }).await
    };
    let release = async {
        host.called("zfs rollback").await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        fs::remove_file(host.path("hold-rollback")).unwrap();
    };
    let (rolled_back, started, ()) = tokio::join!(rollback, start, release);
    assert_eq!((rolled_back, started), (Response::Done, Response::Done));
    let calls = host.calls();
    let back = calls.iter().rposition(|call| call.starts_with("zfs rolled back")).unwrap();
    let start = calls.iter().position(|call| call.ends_with(&format!(" start zendiloos-{ID}"))).unwrap();
    assert!(back < start, "started only once its disks were back: {calls:?}");
}

#[tokio::test]
async fn a_restore_over_a_vm_cut_off_is_undone_before_anything_starts_it() {
    let host = host();
    defined_with(&host, devices(), true).await;
    assert!(host.autostarts(ID));
    let (folder, files) = a_backup(&host).await;
    // The VM went on after its backup.
    let since = vec![4u8; 1 << 20];
    fs::write(host.zvol(ID, 0), &since).unwrap();
    // Its restore writes it all, and the machine goes down before it ends.
    fs::write(host.path("hold-unit-end"), "").unwrap();
    let spec = spec_with(ID, VmFirmware::Uefi, devices(), true);
    let restore = host.run(restore_request(spec, &folder, &files, true, vec![]));
    let written = async {
        host.called("restore-vm").await;
        for _ in 0..400 {
            if holds(&host, 0) != since {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        host.autostarts(ID)
    };
    let autostarts_meanwhile = tokio::select! {
        _ = restore => panic!("the restore ended"),
        autostarts = written => autostarts,
    };
    fs::remove_file(host.path("hold-unit-end")).unwrap();
    assert!(!autostarts_meanwhile, "libvirt does not start it with the machine while its disks are written");
    assert!(holds(&host, 0) != since, "half restored");

    let again = host.restarted();
    again.recover().await;
    assert!(holds(&host, 0) == since, "back to what it was before the restore");
    assert!(host.autostarts(ID), "starting with the machine again");
    let Response::VmSnapshots { zfs_list } = again.execute(&Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(!zfs_list.contains("zdl-2026-11-12_09.00.00"), "the snapshot from before went: {zfs_list}");
    assert_eq!(
        again.execute(&Request::ControlVm { id: vm(), action: VmAction::Start }).await,
        Response::Done
    );
}

#[tokio::test]
async fn a_restore_as_a_new_vm_cut_off_leaves_nothing_of_it() {
    let host = host();
    defined(&host).await;
    let (folder, files) = a_backup(&host).await;
    fs::write(host.path("hold-unit-end"), "").unwrap();
    let copy = spec_with(COPY, VmFirmware::Uefi, devices(), true);
    let restore = host.run(restore_request(copy, &folder, &files, false, vec![]));
    let written = async {
        host.called("restore-vm").await;
        host.autostarts(COPY)
    };
    let autostarts = tokio::select! {
        _ = restore => panic!("the restore ended"),
        autostarts = written => autostarts,
    };
    fs::remove_file(host.path("hold-unit-end")).unwrap();
    assert!(!autostarts, "a VM being restored does not start with the machine");
    assert!(host.zvol(COPY, 0).exists());

    host.restarted().recover().await;
    assert!(!host.zvol(COPY, 0).exists() && !host.zvol(COPY, 1).exists(), "its disks went");
    assert!(!host.path(&format!("state/zendiloos-{COPY}")).exists(), "and its domain");
    assert!(host.zvol(ID, 0).exists(), "the VM it came from stays");
}

#[tokio::test]
async fn a_restore_over_a_vm_writes_nothing_libvirt_would_not_take_and_holds_once_written() {
    let host = host();
    defined(&host).await;
    let (folder, files) = a_backup(&host).await;
    let since = vec![4u8; 1 << 20];
    fs::write(host.zvol(ID, 0), &since).unwrap();
    // libvirt refuses the backup's definition: nothing is written.
    fs::write(host.path("state/define-fails"), "").unwrap();
    let spec = spec_of(ID, VmFirmware::Uefi, devices());
    let failed = host.run(restore_request(spec.clone(), &folder, &files, true, vec![])).await;
    assert!(matches!(failed, Response::Error { .. }), "{failed:?}");
    assert!(holds(&host, 0) == since, "nothing written");
    assert!(!host.calls().iter().any(|call| call.contains("restore-vm")), "the unit never ran");
    fs::remove_file(host.path("state/define-fails")).unwrap();

    // A third disk the backup has not cannot go: the restore holds.
    let three = VmDevices { disks: vec![disk(0, 1), disk(1, 2), disk(2, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, three), remove: vec![] };
    assert_eq!(host.run(change).await, Response::Done);
    fs::write(host.path("busy"), format!("tank/zendiloos.vms/{ID}-disk2\n")).unwrap();
    assert_eq!(host.run(restore_request(spec, &folder, &files, true, vec![slot(2)])).await, Response::Done);
    assert!(holds(&host, 0) != since, "restored");
    assert!(host.zvol(ID, 2).exists(), "the disk that did not go stays");
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(!zfs_list.contains("zdl-2026-11-12_09.00.00"), "{zfs_list}");
}

#[tokio::test]
async fn a_freeze_libvirt_gave_up_on_is_thawed_all_the_same() {
    let host = host();
    defined(&host).await;
    host.set_state(ID, "running");
    fs::write(host.path("state/agent-slow"), "").unwrap();
    let snapshot =
        |at: &str| Request::SnapshotVm { id: vm(), label: label(at), schedules: vec![], note: None };
    assert_eq!(
        host.run(snapshot("zdl-2026-11-12_11.00.00")).await,
        Response::VmSnapshotTaken { consistency: VmConsistency::Crash }
    );
    let calls = host.calls();
    let at = |end: &str| calls.iter().position(|call| call.ends_with(end));
    let freeze = at(&format!("domfsfreeze zendiloos-{ID}")).unwrap();
    let snap = calls.iter().position(|call| call.starts_with("zfs snapshot")).unwrap();
    let thaw = at(&format!("domfsthaw zendiloos-{ID}")).expect("thawed after the freeze that failed");
    assert!(freeze < snap && snap < thaw, "{calls:?}");
    // Without a guest agent there is nothing to thaw.
    fs::remove_file(host.path("state/agent-slow")).unwrap();
    let from = host.calls().len();
    assert_eq!(
        host.run(snapshot("zdl-2026-11-12_12.00.00")).await,
        Response::VmSnapshotTaken { consistency: VmConsistency::Crash }
    );
    assert!(!host.calls()[from..].iter().any(|call| call.contains("domfsthaw")));
}

#[tokio::test]
async fn a_restore_that_fails_leaves_the_uefi_variables_and_the_tpm_as_they_were() {
    let host = host();
    fs::write(host.path("swtpm-binary"), "").unwrap();
    let with_tpm = VmDevices { tpm: true, ..devices() };
    defined_with(&host, with_tpm.clone(), false).await;
    let nvram = host.path(&format!("nvram/zendiloos-{ID}_VARS.fd"));
    let tpm = host.path(&format!("swtpm/{UUID}/tpm2/tpm2-00.permall"));
    fs::create_dir_all(tpm.parent().unwrap()).unwrap();
    fs::write(&nvram, b"uefi variables").unwrap();
    fs::write(&tpm, b"tpm state").unwrap();
    let (folder, files) = a_backup(&host).await;
    fs::write(&nvram, b"variables since").unwrap();
    fs::write(&tpm, b"tpm since").unwrap();
    // The TPM's state in the backup is not the one it wrote.
    let copy = host.path(&format!("volume1/backups/{}/tpm2-00.permall", folder.as_str()));
    fs::remove_file(&copy).unwrap();
    fs::write(&copy, b"tpm other").unwrap();
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o644)).unwrap();
    let spec = spec_of(ID, VmFirmware::Uefi, with_tpm);
    refused(&host.run(restore_request(spec, &folder, &files, true, vec![])).await, "SHA-256 differs");
    assert_eq!(fs::read(&nvram).unwrap(), b"variables since");
    assert_eq!(fs::read(&tpm).unwrap(), b"tpm since");
}

#[tokio::test]
async fn a_restore_waits_for_the_devices_of_the_disks_it_makes() {
    let host = host();
    defined(&host).await;
    let (folder, files) = a_backup(&host).await;
    fs::write(host.path("slow-devices"), "").unwrap();
    let copy = spec_of(COPY, VmFirmware::Uefi, devices());
    assert_eq!(host.run(restore_request(copy, &folder, &files, false, vec![])).await, Response::Done);
    assert!(fs::read(host.zvol(COPY, 0)).unwrap() == holds(&host, 0), "restored");
}

#[tokio::test]
async fn a_libvirt_that_does_not_answer_is_not_taken_for_a_vm_that_is_not_there() {
    let host = host();
    defined(&host).await;
    host.set_state(ID, "running");
    fs::write(host.path("state/libvirt-down"), "").unwrap();
    let snapshot = Request::SnapshotVm {
        id: vm(),
        label: label("zdl-2026-11-12_13.00.00"),
        schedules: vec![],
        note: None,
    };
    let snapshot = host.run(snapshot).await;
    assert!(matches!(snapshot, Response::Error { .. }), "not taken as of a VM that is off: {snapshot:?}");
    let deleted = host.run(Request::DeleteVm { id: vm() }).await;
    assert!(matches!(deleted, Response::Error { .. }), "{deleted:?}");
    assert!(host.zvol(ID, 0).exists() && host.zvol(ID, 1).exists(), "its disks stay");
    fs::remove_file(host.path("state/libvirt-down")).unwrap();
    // libvirt answers, but cannot get a domain it lists: no less a failure.
    fs::write(host.path("state/lookup-fails"), "").unwrap();
    let deleted = host.run(Request::DeleteVm { id: vm() }).await;
    assert!(matches!(deleted, Response::Error { .. }), "{deleted:?}");
    assert!(host.zvol(ID, 0).exists(), "its disks stay");
    fs::remove_file(host.path("state/lookup-fails")).unwrap();
    assert_eq!(host.run(Request::DeleteVm { id: vm() }).await, Response::Done);
    assert!(!host.zvol(ID, 0).exists());
}

#[tokio::test]
async fn a_vm_on_two_pools_is_not_snapshotted_at_two_moments_and_resumes_after() {
    let host = host();
    fs::write(
        host.path("units/volume2-\\x40vms.mount"),
        "[Mount]\nWhat=cold/zendiloos.vms\nWhere=/volume2/@vms\n",
    )
    .unwrap();
    let cold = VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() };
    let two = VmDevices { disks: vec![disk(0, 1), VmDisk { location: cold, ..disk(1, 1) }], ..devices() };
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_of(ID, VmFirmware::Uefi, two) }).await,
        Response::Done
    );
    host.set_state(ID, "running");
    let snapshot =
        |at: &str| Request::SnapshotVm { id: vm(), label: label(at), schedules: vec![], note: None };
    // Neither frozen nor paused: no snapshots of two moments.
    fs::write(host.path("state/suspend-fails"), "").unwrap();
    refused(&host.run(snapshot("zdl-2026-11-12_07.00.00")).await, "one moment");
    assert!(!host.calls().iter().any(|call| call.starts_with("zfs snapshot")));
    fs::remove_file(host.path("state/suspend-fails")).unwrap();
    // Paused, it resumes even if libvirt does not let it at first.
    fs::write(host.path("state/resume-fails"), "").unwrap();
    assert_eq!(
        host.run(snapshot("zdl-2026-11-12_08.00.00")).await,
        Response::VmSnapshotTaken { consistency: VmConsistency::Crash }
    );
    let resumed =
        host.calls().iter().filter(|call| call.ends_with(&format!("resume zendiloos-{ID}"))).count();
    assert_eq!(resumed, 2, "asked again");
}

#[tokio::test]
async fn a_change_libvirt_took_keeps_its_disks_whatever_fails_after() {
    let host = host();
    defined(&host).await;
    // Autostart cannot be set once libvirt took the definition with a disk more.
    fs::write(host.path("state/autostart-fails"), "").unwrap();
    let three = VmDevices { disks: vec![disk(0, 1), disk(1, 2), disk(2, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, three), remove: vec![] };
    assert_eq!(host.run(change).await, Response::Done);
    assert!(host.zvol(ID, 2).exists(), "the disk the domain names now stays");
    fs::remove_file(host.path("state/autostart-fails")).unwrap();
    // Disk 1 cannot go once the domain no longer names it: the change holds.
    fs::write(host.path("busy"), format!("tank/zendiloos.vms/{ID}-disk1\n")).unwrap();
    let without = VmDevices { disks: vec![disk(0, 1), disk(2, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, without), remove: vec![slot(1)] };
    assert_eq!(host.run(change).await, Response::Done);
    let domain = fs::read_to_string(host.path(&format!("state/zendiloos-{ID}.defined"))).unwrap();
    assert!(!domain.contains("-disk1'") && domain.contains("-disk2'"), "the domain names disk 2, not disk 1");
    assert!(host.zvol(ID, 1).exists(), "the disk that did not go stays");
}

#[tokio::test]
async fn a_backup_has_the_disks_the_vm_is_recorded_with() {
    let host = host();
    defined(&host).await;
    // A disk whose removal failed: no longer in the domain, its zvol left.
    fs::write(host.zvol(ID, 2), vec![8u8; 1 << 20]).unwrap();
    let (_, files) = a_backup(&host).await;
    let parts: Vec<VmBackupPart> = files.iter().map(|file| file.part).collect();
    assert_eq!(
        parts,
        [
            VmBackupPart::Disk { slot: slot(0), volsize: 1 << 20 },
            VmBackupPart::Disk { slot: slot(1), volsize: 2 << 20 }
        ]
    );
    // A disk of the record without its zvol: no backup that would lack it.
    refused(&backed_up_with(&host, "20261112-040000", vec![slot(0), slot(1), slot(3)]).await, "disk 3");
    // A disk the domain uses that the record lacks: neither.
    let three = VmDevices { disks: vec![disk(0, 1), disk(1, 2), disk(2, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, three), remove: vec![] };
    assert_eq!(host.run(change).await, Response::Done);
    refused(&backed_up_with(&host, "20261112-050000", vec![slot(0), slot(1)]).await, "uses");
    let Response::VmSnapshots { zfs_list } = host.run(Request::VmSnapshots {}).await else {
        panic!("no list")
    };
    assert!(zfs_list.is_empty(), "no snapshot for a backup refused: {zfs_list}");
}

/// A restore over the VM that writes everything and is cut off before it
/// ends, as by a power cut; what disk 0 held before it.
async fn cut_off_restore(host: &Host, spec: VmSpec) -> Vec<u8> {
    let (folder, files) = a_backup(host).await;
    let since = vec![4u8; 1 << 20];
    fs::write(host.zvol(ID, 0), &since).unwrap();
    fs::write(host.path("hold-unit-end"), "").unwrap();
    let restore = host.run(restore_request(spec, &folder, &files, true, vec![]));
    let written = async {
        host.called("restore-vm").await;
        for _ in 0..400 {
            if holds(host, 0) != since {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    };
    tokio::select! {
        _ = restore => panic!("the restore ended"),
        () = written => {}
    }
    fs::remove_file(host.path("hold-unit-end")).unwrap();
    assert!(holds(host, 0) != since, "half restored");
    since
}

/// What undoes a restore of the VM, while it is not undone.
fn restore_marker(host: &Host) -> PathBuf {
    host.path(&format!("domains/zendiloos-{ID}.restoring"))
}

#[tokio::test]
async fn a_disk_added_where_a_removed_one_was_left_is_a_new_one() {
    let host = host();
    fs::write(
        host.path("units/volume2-\\x40vms.mount"),
        "[Mount]\nWhat=cold/zendiloos.vms\nWhere=/volume2/@vms\n",
    )
    .unwrap();
    defined(&host).await;
    let old = holds(&host, 1);
    // Disk 1 does not go as it is removed: the change holds, its zvol stays.
    fs::write(host.path("busy"), format!("tank/zendiloos.vms/{ID}-disk1\n")).unwrap();
    let without = VmDevices { disks: vec![disk(0, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, without), remove: vec![slot(1)] };
    assert_eq!(host.run(change).await, Response::Done);
    assert!(host.zvol(ID, 1).exists());
    // A disk added in its slot is a new one, never the old one with its data.
    let with = VmDevices { disks: vec![disk(0, 1), disk(1, 2)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, with.clone()), remove: vec![] };
    refused(&host.run(change.clone()).await, "left from a disk removed");
    assert!(holds(&host, 1) == old, "nothing taken over");
    let domain = fs::read_to_string(host.path(&format!("state/zendiloos-{ID}.defined"))).unwrap();
    assert!(!domain.contains("-disk1'"), "the domain still has no disk 1");
    fs::remove_file(host.path("busy")).unwrap();
    assert_eq!(host.run(change).await, Response::Done);
    assert!(holds(&host, 1).iter().all(|b| *b == 0), "a new, empty disk");
    // Left on one pool, added on another: a new disk there.
    fs::write(host.path("busy"), format!("tank/zendiloos.vms/{ID}-disk1\n")).unwrap();
    let without = VmDevices { disks: vec![disk(0, 1)], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, without), remove: vec![slot(1)] };
    assert_eq!(host.run(change).await, Response::Done);
    fs::remove_file(host.path("busy")).unwrap();
    let cold = VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() };
    let elsewhere =
        VmDevices { disks: vec![disk(0, 1), VmDisk { location: cold, ..disk(1, 2) }], ..devices() };
    let change = Request::UpdateVm { spec: spec_of(ID, VmFirmware::Uefi, elsewhere), remove: vec![] };
    assert_eq!(host.run(change).await, Response::Done);
    assert!(!host.zvol(ID, 1).exists(), "the one left went");
    assert!(host.path(&format!("zvol/cold/zendiloos.vms/{ID}-disk1")).exists());
}

#[tokio::test]
async fn a_restore_cut_off_is_not_undone_while_its_pool_is_away_and_is_once_it_is_back() {
    let host = host();
    defined(&host).await;
    let since = cut_off_restore(&host, spec_of(ID, VmFirmware::Uefi, devices())).await;
    fs::write(host.path("pool-gone"), "").unwrap();
    let again = host.restarted();
    again.recover().await;
    assert!(restore_marker(&host).exists(), "kept while its disks cannot be seen");
    let start = again.execute(&Request::ControlVm { id: vm(), action: VmAction::Start }).await;
    assert!(matches!(start, Response::Error { .. }), "{start:?}");
    assert!(holds(&host, 0) != since, "nothing was rolled back");
    fs::remove_file(host.path("pool-gone")).unwrap();
    assert_eq!(
        again.execute(&Request::ControlVm { id: vm(), action: VmAction::Start }).await,
        Response::Done
    );
    assert!(holds(&host, 0) == since, "undone once the pool is back, before the start");
    assert!(!restore_marker(&host).exists());
}

#[tokio::test]
async fn a_restore_cut_off_is_undone_only_once_its_unit_stopped_and_its_vm_is_off() {
    let host = host();
    defined(&host).await;
    let since = cut_off_restore(&host, spec_of(ID, VmFirmware::Uefi, devices())).await;
    // Its unit does not stop: nothing is undone under what it writes.
    fs::write(host.path("unit-stuck"), "").unwrap();
    host.restarted().recover().await;
    assert!(restore_marker(&host).exists() && holds(&host, 0) != since, "not undone under its unit");
    fs::remove_file(host.path("unit-stuck")).unwrap();
    // Started meanwhile: nothing is rolled back under the running guest.
    host.set_state(ID, "running");
    host.restarted().recover().await;
    assert!(restore_marker(&host).exists() && holds(&host, 0) != since, "not undone while it runs");
    host.set_state(ID, "shut off");
    host.restarted().recover().await;
    assert!(!restore_marker(&host).exists() && holds(&host, 0) == since);
}

#[tokio::test]
async fn a_restore_turns_autostart_off_before_it_marks_itself_and_gives_back_libvirts() {
    let host = host();
    // libvirt starts it with the machine, its record says it does not.
    defined_with(&host, devices(), true).await;
    cut_off_restore(&host, spec_of(ID, VmFirmware::Uefi, devices())).await;
    assert!(!host.autostarts(ID));
    assert!(!host.path("state/disabled-after-marking").exists(), "off before the restore marked itself");
    host.restarted().recover().await;
    assert!(host.autostarts(ID), "as libvirt had it before");
}

#[tokio::test]
async fn a_delete_that_fails_keeps_what_undoes_a_restore_cut_off() {
    let host = host();
    defined(&host).await;
    cut_off_restore(&host, spec_of(ID, VmFirmware::Uefi, devices())).await;
    fs::write(host.path("busy"), format!("tank/zendiloos.vms/{ID}-disk1\n")).unwrap();
    let deleted = host.run(Request::DeleteVm { id: vm() }).await;
    assert!(matches!(deleted, Response::Error { .. }), "{deleted:?}");
    assert!(restore_marker(&host).exists(), "kept while the VM is there");
    fs::remove_file(host.path("busy")).unwrap();
    assert_eq!(host.run(Request::DeleteVm { id: vm() }).await, Response::Done);
    assert!(!restore_marker(&host).exists());
}
