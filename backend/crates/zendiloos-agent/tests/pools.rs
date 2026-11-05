//! The pool operations against a fake host: devices, sysfs, mountinfo and
//! swaps in a temporary directory, programs replaced by scripts that record
//! their arguments.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{AccountFiles, Erasable, ErasePolicy, HostPaths, Operations, Paths};
use zendiloos_agent_proto::{
    DiskId, ErrorCode, GroupName, Layout, Network, NfsClient, NfsExport, PoolName, Principal, Request,
    Response, Role, ShareAccess, ShareName, SmbShare, SnapshotFrequency, SnapshotLabel, SnapshotNote,
    VolumeNumber,
};

/// 8 GiB in 512-byte sectors.
const DISK_SECTORS: u64 = 16_777_216;

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
}

/// Block device `name` with major:minor `number`, under `parent` if it is
/// a partition.
fn device(root: &Path, name: &str, number: &str, parent: Option<&str>, sectors: u64) {
    fs::write(root.join("dev").join(name), b"").unwrap();
    let sys = match parent {
        Some(disk) => root.join("sys").join(disk).join(name),
        None => root.join("sys").join(name),
    };
    fs::create_dir_all(sys.join("holders")).unwrap();
    fs::write(sys.join("dev"), format!("{number}\n")).unwrap();
    fs::write(sys.join("size"), format!("{sectors}\n")).unwrap();
    fs::write(sys.join("ro"), "0\n").unwrap();
    if let Some(disk) = parent {
        fs::write(sys.join("partition"), "1\n").unwrap();
        // /sys/class/block lists partitions at the top level too.
        symlink(&sys, root.join("sys").join(name)).unwrap();
        let _ = disk;
    }
}

fn link(root: &Path, id: &str, target: &str) {
    symlink(format!("../../{target}"), root.join("dev/disk/by-id").join(id)).unwrap();
}

/// Free disks sda and sdb; sdc with a mounted partition; sdd held by md0;
/// sde read-only; sdf the member of pool `tank` (volume 1).
fn host(erasable: Erasable) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in ["dev/disk/by-id", "sys", "units"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    device(&root, "sda", "8:0", None, DISK_SECTORS);
    device(&root, "sdb", "8:16", None, DISK_SECTORS);
    device(&root, "sdc", "8:32", None, DISK_SECTORS);
    device(&root, "sdc1", "8:33", Some("sdc"), DISK_SECTORS - 4096);
    device(&root, "sdd", "8:48", None, DISK_SECTORS);
    fs::write(root.join("sys/sdd/holders/md0"), b"").unwrap();
    device(&root, "sde", "8:64", None, DISK_SECTORS);
    fs::write(root.join("sys/sde/ro"), "1\n").unwrap();
    device(&root, "sdf", "8:80", None, DISK_SECTORS);
    device(&root, "sdf1", "8:81", Some("sdf"), 16_515_072);
    for (id, target) in [
        ("ata-FREE-1", "sda"),
        ("ata-FREE-2", "sdb"),
        ("ata-MOUNTED", "sdc"),
        ("ata-PARTITION", "sdc1"),
        ("ata-HELD", "sdd"),
        ("ata-READONLY", "sde"),
        ("ata-MEMBER", "sdf"),
        ("ata-MEMBER-part1", "sdf1"),
        // What udev creates once sgdisk has made the partitions.
        ("ata-FREE-1-part1", "sda"),
        ("ata-FREE-2-part1", "sdb"),
    ] {
        link(&root, id, target);
    }
    fs::write(root.join("mountinfo"), "22 1 8:33 / /srv/old rw,relatime shared:1 - ext4 /dev/sdc1 rw\n")
        .unwrap();
    fs::write(root.join("swaps"), "Filename\tType\tSize\tUsed\tPriority\n").unwrap();
    fs::write(root.join("units/volume1.mount"), "[Mount]\n").unwrap();
    fs::write(root.join("units/volume1-old\\x2dshare.mount"), "[Mount]\n").unwrap();
    fs::create_dir_all(root.join("volume1/photos")).unwrap();

    let log = root.join("calls.log");
    let by_id = root.join("dev/disk/by-id");
    let script = |name: &str, body: &str| {
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let tools = Tools {
        zpool: script(
            "zpool",
            &format!(
                "case \"$1 $2 $3\" in\n\
                 'list -H -o') echo tank ;;\n\
                 'list -H -P') printf 'tank\\t8G\\n\\t{}/ata-MEMBER-part1\\t8G\\n' ;;\n\
                 esac",
                by_id.display()
            ),
        ),
        zfs: script(
            "zfs",
            "case \"$*\" in\n\
             *'-s local -d 0'*) echo 1 ;;\n\
             *'-o name -d 1 tank') printf 'tank\\ntank/photos\\n' ;;\n\
             *' tank') echo 1 ;;\n\
             *) echo - ;;\n\
             esac",
        ),
        sgdisk: script("sgdisk", ""),
        wipefs: script("wipefs", ""),
        udevadm: script("udevadm", ""),
        systemctl: script("systemctl", ""),
        testparm: script("testparm", ""),
        exportfs: script("exportfs", ""),
        setfacl: script("setfacl", ""),
        ..Tools::default()
    };
    let paths = Paths {
        accounts: AccountFiles::default(),
        host: HostPaths {
            dev: root.join("dev"),
            by_id,
            sys_block: root.join("sys"),
            mountinfo: root.join("mountinfo"),
            swaps: root.join("swaps"),
            units: root.join("units"),
            volumes: root.clone(),
            samba_config: root.join("samba/zendiloos.conf"),
            nfs_exports: root.join("exports.d/zendiloos.exports"),
            console_issue: root.join("zendiloos.issue"),
        },
    };
    let operations = Operations::with_paths(Role::System, &tools, erasable, paths).unwrap();
    Host { _dir: dir, root, operations }
}

fn ids(list: &[&str]) -> Vec<DiskId> {
    list.iter().map(|id| DiskId::parse(id).unwrap()).collect()
}

fn create(name: &str, disks: &[&str], volume: u16) -> Request {
    Request::CreatePool {
        name: PoolName::parse(name).unwrap(),
        layout: Layout::Mirror,
        disks: ids(disks),
        volume: VolumeNumber::new(volume).unwrap(),
    }
}

fn assert_refused(response: Response, code: ErrorCode, contains: &str) {
    match response {
        Response::Error { code: actual, message } => {
            assert_eq!(actual, code, "{message}");
            assert!(message.contains(contains), "{message:?} lacks {contains:?}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test]
async fn creates_a_mirror_with_equal_partitions_and_a_mount_unit() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));

    let response = host.run(create("media", &["ata-FREE-1", "ata-FREE-2"], 2)).await;

    assert_eq!(response, Response::Done);
    let calls = host.calls();
    // 8 GiB minus 1 MiB in front and the 128 MiB reserve.
    assert!(
        calls.contains(
            &"sgdisk --new=1:1M:+8063M --typecode=1:BF01 --change-name=1:zendiloos /dev/sda".to_owned()
        ),
        "{calls:#?}"
    );
    let create = calls.iter().find(|c| c.starts_with("zpool create")).unwrap();
    assert_eq!(
        create,
        "zpool create -o ashift=12 -O compression=lz4 -O atime=off -O xattr=sa -O acltype=posixacl \
         -O dnodesize=auto -O normalization=formD -O utf8only=on -O setuid=off -O devices=off \
         -O mountpoint=legacy -O canmount=on \
         -O zendiloos:volume=2 media mirror /dev/disk/by-id/ata-FREE-1-part1 /dev/disk/by-id/ata-FREE-2-part1"
    );
    assert_eq!(
        &calls[calls.len() - 2..],
        ["systemctl daemon-reload", "systemctl enable --now volume2.mount"]
    );
    let unit = fs::read_to_string(host.path("units/volume2.mount")).unwrap();
    assert!(unit.contains("What=media\nWhere=/volume2\nType=zfs\n"), "{unit}");
    assert!(unit.contains("After=zfs-import.target"), "{unit}");
}

#[tokio::test]
async fn never_erases_a_disk_in_use() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    for (disk, reason) in [
        ("ata-MOUNTED", "mounted at /srv/old"),
        ("ata-HELD", "in use by md0"),
        ("ata-READONLY", "read-only"),
        ("ata-PARTITION", "not a whole disk"),
        ("ata-GONE", "not present"),
    ] {
        let response = host.run(create("media", &["ata-FREE-1", disk], 2)).await;
        assert_refused(response, ErrorCode::Forbidden, reason);
    }
    let twice = host.run(create("media", &["ata-FREE-1", "ata-FREE-1"], 2)).await;
    assert_refused(twice, ErrorCode::Forbidden, "listed twice");
    let one = host.run(create("media", &["ata-FREE-1"], 2)).await;
    assert_refused(one, ErrorCode::Forbidden, "takes");
    assert_eq!(host.calls(), Vec::<String>::new(), "nothing ran");
}

#[tokio::test]
async fn takes_neither_a_used_name_nor_a_used_volume() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    assert_refused(host.run(create("tank", &["ata-FREE-1", "ata-FREE-2"], 2)).await, ErrorCode::Conflict, "");
    assert_refused(
        host.run(create("media", &["ata-FREE-1", "ata-FREE-2"], 1)).await,
        ErrorCode::Conflict,
        "",
    );
    assert!(
        host.calls().iter().all(|c| !c.starts_with("sgdisk") && !c.starts_with("wipefs")),
        "nothing erased"
    );
}

#[tokio::test]
async fn erases_only_what_the_configuration_allows() {
    let none = host(Erasable::Policy(ErasePolicy::None));
    assert_refused(
        none.run(create("media", &["ata-FREE-1", "ata-FREE-2"], 2)).await,
        ErrorCode::Forbidden,
        "disabled",
    );

    let listed = host(Erasable::Only(vec!["ata-FREE-1".to_owned()]));
    let response = listed.run(create("media", &["ata-FREE-1", "ata-FREE-2"], 2)).await;
    assert_refused(response, ErrorCode::Forbidden, "ata-FREE-2 is not in the list");
    assert_eq!(listed.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn destroys_only_its_own_pools_and_erases_their_disks() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let destroy = |name: &str, volume| Request::DestroyPool {
        name: PoolName::parse(name).unwrap(),
        volume: VolumeNumber::new(volume).unwrap(),
    };
    assert_refused(host.run(destroy("tank", 2)).await, ErrorCode::Forbidden, "not managed");
    assert_refused(host.run(destroy("other", 1)).await, ErrorCode::Forbidden, "not managed");
    assert!(!host.calls().iter().any(|c| c.starts_with("zpool destroy")));

    assert_eq!(host.run(destroy("tank", 1)).await, Response::Done);
    let calls = host.calls();
    let share = calls.iter().position(|c| c == "systemctl disable --now volume1-old\\x2dshare.mount");
    let from = calls.iter().position(|c| c == "systemctl disable --now volume1.mount").expect("unmounted");
    assert!(share.is_some_and(|share| share < from), "shares unmount first: {calls:#?}");
    assert!(!host.path("units/volume1-old\\x2dshare.mount").exists());
    assert_eq!(
        &calls[from..],
        [
            "systemctl disable --now volume1.mount",
            "systemctl daemon-reload",
            "zpool destroy tank",
            "zpool labelclear -f /dev/disk/by-id/ata-MEMBER-part1",
            "wipefs --all --quiet /dev/disk/by-id/ata-MEMBER-part1",
            "wipefs --all --quiet /dev/sdf",
            "sgdisk --zap-all /dev/sdf",
        ]
    );
    assert!(!host.path("units/volume1.mount").exists());
}

#[tokio::test]
async fn replaces_a_device_with_a_partition_of_the_members_size() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let request = Request::ReplaceDisk {
        pool: PoolName::parse("tank").unwrap(),
        device: 4_933_705_814_018_298_948,
        disk: DiskId::parse("ata-FREE-2").unwrap(),
    };

    assert_eq!(host.run(request).await, Response::Done);

    let calls = host.calls();
    // sdf1 has 16 515 072 sectors: 8064 MiB.
    assert!(
        calls.contains(
            &"sgdisk --new=1:1M:+8064M --typecode=1:BF01 --change-name=1:zendiloos /dev/sdb".to_owned()
        ),
        "{calls:#?}"
    );
    assert_eq!(
        calls.last().unwrap(),
        "zpool replace tank 4933705814018298948 /dev/disk/by-id/ata-FREE-2-part1"
    );

    let busy = Request::ReplaceDisk {
        pool: PoolName::parse("tank").unwrap(),
        device: 1,
        disk: DiskId::parse("ata-MOUNTED").unwrap(),
    };
    assert_refused(host.run(busy).await, ErrorCode::Forbidden, "mounted");
}

fn share(name: &str) -> ShareName {
    ShareName::parse(name).unwrap()
}

#[tokio::test]
async fn shares_are_datasets_with_their_own_mount_unit() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let create = |name: &str, pool: &str| Request::CreateShare {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        name: share(name),
    };
    assert_eq!(host.run(create("my-photos", "tank")).await, Response::Done);
    let calls = host.calls();
    assert!(
        calls.contains(
            &"zfs create -o mountpoint=legacy -o setuid=off -o devices=off tank/my-photos".to_owned()
        ),
        "{calls:#?}"
    );
    assert_eq!(calls.last().unwrap(), "systemctl enable --now volume1-my\\x2dphotos.mount");
    let unit = fs::read_to_string(host.path("units/volume1-my\\x2dphotos.mount")).unwrap();
    assert!(unit.contains("What=tank/my-photos\nWhere=/volume1/my-photos\nType=zfs\n"), "{unit}");

    assert_refused(host.run(create("other", "foreign")).await, ErrorCode::Forbidden, "not managed");

    let destroy = Request::DestroyShare {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        name: share("my-photos"),
    };
    assert_eq!(host.run(destroy).await, Response::Done);
    let calls = host.calls();
    assert_eq!(calls.last().unwrap(), "zfs destroy -r tank/my-photos");
    let flushed = calls
        .iter()
        .position(|c| c == "systemctl start zendiloos-nfs-flush.service")
        .expect("the NFS server lets go first");
    let unmounted = calls.iter().position(|c| c.starts_with("systemctl disable --now volume1-my")).unwrap();
    assert!(flushed < unmounted, "{calls:#?}");
    assert!(!host.path("units/volume1-my\\x2dphotos.mount").exists());
}

#[tokio::test]
async fn writes_samba_and_nfs_configuration_after_checking_it() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let smb = vec![SmbShare {
        name: share("photos"),
        volume: VolumeNumber::new(1).unwrap(),
        access: vec![ShareAccess {
            principal: Principal::Group(GroupName::parse("family").unwrap()),
            write: true,
        }],
        recycle_bin: false,
        comment: None,
    }];
    let nfs = vec![NfsExport {
        name: share("photos"),
        volume: VolumeNumber::new(1).unwrap(),
        clients: vec![NfsClient { network: Network::parse("192.168.1.0/24").unwrap(), write: false }],
    }];

    assert_eq!(host.run(Request::ApplySharing { smb, nfs }).await, Response::Done);

    let samba = fs::read_to_string(host.path("samba/zendiloos.conf")).unwrap();
    assert!(samba.contains("[photos]\n   path = /volume1/photos\n   valid users = @family\n"), "{samba}");
    let exports = fs::read_to_string(host.path("exports.d/zendiloos.exports")).unwrap();
    assert!(exports.contains("/volume1/photos 192.168.1.0/24(ro,"), "{exports}");
    assert_eq!(
        host.calls(),
        [
            "testparm --suppress-prompt /samba/zendiloos.check",
            "systemctl try-reload-or-restart smbd.service",
            "systemctl reload-or-restart nfs-server.service",
        ]
    );
}

#[tokio::test]
async fn each_agent_serves_only_its_role() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let access = Request::SetShareAccess {
        volume: VolumeNumber::new(1).unwrap(),
        name: share("photos"),
        access: vec![],
        removed: vec![],
        recursive: false,
    };
    assert_refused(host.run(access).await, ErrorCode::Forbidden, "not for the System agent");
}

#[tokio::test]
async fn snapshots_are_of_one_share_and_destroy_nothing_else() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let label = || SnapshotLabel::parse("zdl-2026-11-05_19.40.01").unwrap();
    let take = |share_name: &str, pool: &str| Request::CreateSnapshot {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        share: share(share_name),
        label: label(),
        schedules: vec![SnapshotFrequency::Hourly, SnapshotFrequency::Daily],
        note: Some(SnapshotNote::parse("before the move").unwrap()),
    };

    assert_eq!(host.run(take("photos", "tank")).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        "zfs snapshot -o zendiloos:schedule=hourly,daily -o zendiloos:note=before the move tank/photos@zdl-2026-11-05_19.40.01"
    );
    assert_refused(host.run(take("nothing", "tank")).await, ErrorCode::Forbidden, "no share");
    assert_refused(host.run(take("photos", "foreign")).await, ErrorCode::Forbidden, "not managed");

    let destroy = Request::DestroySnapshot {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        share: share("photos"),
        label: label(),
    };
    assert_eq!(host.run(destroy).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), "zfs destroy tank/photos@zdl-2026-11-05_19.40.01");

    assert!(matches!(host.run(Request::ListSnapshots {}).await, Response::Snapshots { .. }));
    assert_eq!(
        host.calls().last().unwrap(),
        "zfs list -H -p -t snapshot -o name,creation,used,referenced,zendiloos:schedule,zendiloos:note"
    );
}

#[tokio::test]
async fn scrubs_only_its_own_pools() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let pool = |name: &str| PoolName::parse(name).unwrap();

    assert_eq!(host.run(Request::ScrubPool { pool: pool("tank") }).await, Response::Done);
    assert_eq!(host.run(Request::StopScrub { pool: pool("tank") }).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), "zpool scrub -s tank");
    for request in [Request::ScrubPool { pool: pool("rpool") }, Request::StopScrub { pool: pool("rpool") }] {
        assert_refused(host.run(request).await, ErrorCode::Forbidden, "not managed");
    }
}
