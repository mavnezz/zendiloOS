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
    VmBackupPart, VmBackupSettings, VmBoot, VmConsistency, VmCpus, VmDevices, VmDisk, VmDiskBus, VmDiskSize,
    VmDiskSlot, VmFirmware, VmId, VmMemory, VmNetwork, VmNic, VmNicModel, VmNicSlot, VmOs, VmSpec, VmStorage,
    VmUsbDevice, VolumeNumber,
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
/// its zvol has `snapdev=visible`.
const ZFS: &str = r#"
root={root}
eval last=\${$#}
flat() { echo "$1" | tr / +; }
case "$*" in
  'list -H -o name tank/zendiloos.vms'|'list -H -o name cold/zendiloos.vms') exit 0 ;;
  'list -H -o name -t snapshot '*) test -f "$root/snaps/$(flat "$last")" ;;
  'list -H -o name -s createtxg -t snapshot -d 1 '*)
      grep "^$last@" "$root/snaps/order" 2>/dev/null; exit 0 ;;
  'list -H -o name '*/zendiloos.vms/*) test -f "$root/zvol/$last" ;;
  'list -H -p -r -t snapshot '*)
      while read -r snap; do [ -f "$root/snaps/$(flat "$snap")" ] || continue
        printf '%s\t1793000000\t4096\t8192\t%s\t%s\t%s\n' "$snap" "$(cat "$root/snaps/$(flat "$snap").schedule" 2>/dev/null || echo -)" \
          "$(cat "$root/snaps/$(flat "$snap").note" 2>/dev/null || echo -)" "$(cat "$root/snaps/$(flat "$snap").consistency")"
      done < "$root/snaps/order" 2>/dev/null; exit 0 ;;
  'get -H -p -o value volsize '*) [ -f "$root/zvol/$last" ] && stat -c %s "$root/zvol/$last" ;;
  'create -s -V '*) mkdir -p "$(dirname "$root/zvol/$last")"; truncate -s "${4%G}M" "$root/zvol/$last" ;;
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
  'destroy -r '*) rm -f "$root/zvol/$last"; for s in "$root"/snaps/$(flat "$last")@*; do rm -f "$s"; done ;;
  'destroy '*@*) [ -f "$root/snaps/$(flat "$last")" ] || exit 1; rm -f "$root/snaps/$(flat "$last")"* "$root/zvol/$last"
      grep -v "^$last\$" "$root/snaps/order" > "$root/snaps/order.new"; mv "$root/snaps/order.new" "$root/snaps/order" ;;
  'destroy '*) rm -f "$root/zvol/$last" ;;
  'rollback '*) cp "$root/snaps/$(flat "$last")" "$root/zvol/${last%@*}"
      if [ "$2" = -r ]; then after=$(sed -n "\|^$last\$|,\$p" "$root/snaps/order" | tail -n +2 | grep "^${last%@*}@")
        for s in $after; do rm -f "$root/snaps/$(flat "$s")"*; grep -v "^$s\$" "$root/snaps/order" > "$root/snaps/order.new"; mv "$root/snaps/order.new" "$root/snaps/order"; done; fi ;;
esac
"#;

/// Domains are files below `state/` holding what they do; the guest agent
/// answers if `state/agent` is there.
const VIRSH: &str = r#"
state={root}/state
shift 2
case "$1" in
  domstate) test -f "$state/$2" && cat "$state/$2" ;;
  define) name=$(sed -n 's/.*<name>\(.*\)<\/name>.*/\1/p' "$3"); cp "$3" "$state/$name.defined"
          [ -f "$state/$name" ] || echo 'shut off' > "$state/$name" ;;
  domuuid) test -f "$state/$2" && echo {uuid} ;;
  start) echo running > "$state/${2#--reset-nvram}"; [ "$2" = --reset-nvram ] && echo running > "$state/$3"; true ;;
  domfsfreeze) test -f "$state/agent" ;;
  domfsthaw|suspend|resume|autostart|attach-device|detach-device) exit 0 ;;
  undefine) eval last=\${$#}; rm -f "$state/$last" ;;
esac
"#;

/// Units: what `systemd-run` is asked runs here, as the unit would.
const SYSTEMD_RUN: &str = r#"
while [ "${1#--}" != "$1" ]; do shift; done
exec "$@"
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
    let tools = Tools {
        zfs: script(&root, "zfs", &fill(ZFS)),
        virsh: script(&root, "virsh", &fill(VIRSH)),
        systemd_run: script(&root, "systemd-run", SYSTEMD_RUN),
        systemctl: script(&root, "systemctl", "[ \"$1\" = show ] && echo inactive; exit 0"),
        udevadm: script(&root, "udevadm", ""),
        busctl: script(&root, "busctl", "exit 1"),
        libvirtd: root.join("libvirtd"),
        qemu: root.join("qemu"),
        swtpm: root.join("swtpm-binary"),
        ..Tools::default()
    };
    fs::write(&tools.libvirtd, "").unwrap();
    fs::write(&tools.qemu, "").unwrap();
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
    let operations =
        Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
    Host { _dir: dir, root, operations }
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
    VmSpec::new(
        VmId::parse(id).unwrap(),
        VmOs::Windows,
        VmCpus::new(2).unwrap(),
        VmMemory::new(1024).unwrap(),
        firmware,
        devices,
        false,
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
    let spec = spec_of(ID, VmFirmware::Uefi, devices());
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
    host.run(Request::BackUpVm {
        id: vm(),
        root: share(),
        folder: SharePath::parse("vms").unwrap(),
        stem: BackupStem::parse("20261112-030000").unwrap(),
        label: label("zdl-2026-11-12_03.00.00"),
        settings: settings(),
    })
    .await
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
