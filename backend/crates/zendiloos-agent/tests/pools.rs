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
    AppName, DiskId, ErrorCode, FileServices, GroupName, Layout, NfsClient, NfsExport, NfsHost, PoolName,
    Principal, Request, Response, Role, ShareAccess, ShareCompression, ShareName, SharePermission,
    ShareRecordSize, SmbProtocol, SmbShare, SnapshotFrequency, SnapshotLabel, SnapshotNote, VolumeNumber,
    Workgroup,
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
    fs::write(
        root.join("passwd"),
        "root:x:0:0::/root:/bin/bash\nanna:x:2000:990::/nonexistent:/usr/sbin/nologin\n\
         zendiloos-files:x:995:995::/:/usr/sbin/nologin\n",
    )
    .unwrap();
    fs::write(root.join("group"), "root:x:0:\nzendiloos-users:x:990:\nzendiloos-files:x:995:\n").unwrap();

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
        // `busy` makes the volume's mount busy, as an open file would.
        systemctl: script(
            "systemctl",
            &format!(
                "[ -f {} ] && [ \"$1 $2\" = 'stop volume1.mount' ] && {{ echo 'target is busy' >&2; exit 1; }}\nexit 0",
                root.join("busy").display()
            ),
        ),
        testparm: script("testparm", ""),
        exportfs: script("exportfs", ""),
        smbcontrol: script("smbcontrol", ""),
        busctl: script("busctl", ""),
        ip: "/bin/false".into(),
        hostnamectl: "/bin/false".into(),
        timedatectl: script("timedatectl", ""),
        sshd: script("sshd", ""),
        systemd_run: script("systemd-run", ""),
        docker: script("docker", ""),
        dockerd: script("dockerd", ""),
        useradd: script("useradd", ""),
        groupadd: script("groupadd", ""),
        userdel: script("userdel", ""),
        groupdel: script("groupdel", ""),
        ..Tools::default()
    };
    let paths = Paths {
        network: Default::default(),
        time: Default::default(),
        host_name: Default::default(),
        configuration: Default::default(),
        ssh: Default::default(),
        web: Default::default(),
        acme: Default::default(),
        web_server: Default::default(),
        firewall: Default::default(),
        vms: Default::default(),
        backup: Default::default(),
        accounts: AccountFiles {
            passwd: root.join("passwd"),
            group: root.join("group"),
            high_water: root.join("id-high-water"),
        },
        host: HostPaths {
            dev: root.join("dev"),
            by_id,
            sys_block: root.join("sys"),
            dmi: root.join("dmi"),
            mountinfo: root.join("mountinfo"),
            swaps: root.join("swaps"),
            units: root.join("units"),
            volumes: root.clone(),
            samba_config: root.join("samba/zendiloos.conf"),
            nfs_exports: root.join("exports.d/zendiloos.exports"),
            console_issue: root.join("zendiloos.issue"),
            docker_config: root.join("etc-docker/daemon.json"),
            apps: root.join("etc-apps"),
            projects: root.join("etc-projects"),
            localtime: root.join("localtime"),
            app_shares: root.join("app-shares"),
            project_shares: root.join("project-shares"),
            removable: root.join("removable"),
            agent: root.join("zendiloos-agent"),
            packages: root.join("packages"),
            docker_socket: root.join("docker.sock"),
            update_spool: root.join("updates"),
            release_keys: vec![root.join("release-signing.pub")],
            share_mounts: true,
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
    // Linked first: the new unit's file and link take one reload.
    assert_eq!(
        &calls[calls.len() - 3..],
        [
            "systemctl enable --no-reload volume2.mount",
            "systemctl daemon-reload",
            "systemctl start volume2.mount"
        ]
    );
    let unit = fs::read_to_string(host.path("units/volume2.mount")).unwrap();
    assert!(unit.contains("What=media\nWhere=/volume2\nType=zfs\n"), "{unit}");
    assert!(unit.contains("After=zfs-import.target"), "{unit}");
}

#[tokio::test]
async fn a_pools_making_reports_its_steps_to_the_client() {
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, Mutex};
    use zendiloos_agent::{Server, serve};
    use zendiloos_agent_proto::{AgentTransport, UnixTransport};

    let Host { _dir, root, operations } = host(Erasable::Policy(ErasePolicy::Unused));
    let socket = root.join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let our_uid = fs::metadata(&root).unwrap().uid();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(listener, Server::new(operations, our_uid), async {
        let _ = stopped.await;
    }));
    let steps = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&steps);
    let report =
        move |step: &str, percent: Option<u8>| heard.lock().unwrap().push((step.to_owned(), percent));

    let client = UnixTransport::new(&socket, std::time::Duration::from_secs(30));
    let response = client
        .call_reporting(create("media", &["ata-FREE-1", "ata-FREE-2"], 2), None, &report)
        .await
        .unwrap();

    assert_eq!(response, Response::Done);
    let steps = steps.lock().unwrap().clone();
    let expected = [
        ("preparing_disks", Some(0)),
        ("preparing_disks", Some(50)),
        ("creating_pool", None),
        ("mounting", None),
    ];
    assert_eq!(
        steps,
        expected.map(|(step, percent)| (step.to_owned(), percent)),
        "in order, before the answer"
    );
    let _ = stop.send(());
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
    let from =
        calls.iter().position(|c| c == "systemctl stop volume1-old\\x2dshare.mount").expect("unmounted");
    assert!(!host.path("units/volume1-old\\x2dshare.mount").exists());
    assert_eq!(
        &calls[from..],
        [
            // Every mount stopped first, the shares' before the volume's.
            "systemctl stop volume1-old\\x2dshare.mount",
            "systemctl stop volume1.mount",
            "zpool destroy tank",
            // Only then their units go.
            "systemctl disable --no-reload --now volume1-old\\x2dshare.mount",
            "systemctl daemon-reload",
            "systemctl disable --no-reload --now volume1.mount",
            "systemctl daemon-reload",
            "zpool labelclear -f /dev/disk/by-id/ata-MEMBER-part1",
            "wipefs --all --quiet /dev/disk/by-id/ata-MEMBER-part1",
            "wipefs --all --quiet /dev/sdf",
            "sgdisk --zap-all /dev/sdf",
        ]
    );
    assert!(!host.path("units/volume1.mount").exists());
}

#[tokio::test]
async fn a_busy_mount_leaves_the_pool_and_its_mounts_as_they_were() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::write(host.path("busy"), "").unwrap();
    let destroy = Request::DestroyPool {
        name: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
    };

    let Response::Error { message, .. } = host.run(destroy).await else { panic!("destroyed") };

    assert!(message.contains("busy"), "{message}");
    let calls = host.calls();
    let from = calls.iter().position(|c| c == "systemctl stop volume1-old\\x2dshare.mount").expect("tried");
    assert_eq!(
        &calls[from..],
        [
            "systemctl stop volume1-old\\x2dshare.mount",
            "systemctl stop volume1.mount",
            "systemctl start volume1-old\\x2dshare.mount",
        ]
    );
    assert!(
        host.path("units/volume1.mount").exists() && host.path("units/volume1-old\\x2dshare.mount").exists()
    );
}

#[tokio::test]
async fn a_pool_with_dockers_data_is_not_destroyed() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    let data_root = host.path("volume1/@docker");
    fs::write(host.path("etc-docker/daemon.json"), format!("{{\"data-root\": \"{}\"}}", data_root.display()))
        .unwrap();
    let destroy = Request::DestroyPool {
        name: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
    };

    assert_refused(host.run(destroy).await, ErrorCode::Forbidden, "Docker's data");
    assert!(!host.calls().iter().any(|c| c.starts_with("zpool destroy") || c.starts_with("systemctl stop")));
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
    assert_eq!(calls.last().unwrap(), "systemctl start volume1-my\\x2dphotos.mount");
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
    let unmounted =
        calls.iter().position(|c| c.starts_with("systemctl disable --no-reload --now volume1-my")).unwrap();
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
            permission: SharePermission::ReadWrite,
        }],
        comment: None,
        time_machine: None,
    }];
    let nfs = vec![NfsExport {
        name: share("photos"),
        volume: VolumeNumber::new(1).unwrap(),
        clients: vec![NfsClient {
            network: NfsHost::parse("192.168.1.0/24").unwrap(),
            write: false,
            asynchronous: false,
            insecure: false,
        }],
    }];

    let request = Request::ApplySharing { smb, nfs, services: services(true), homes: None, close: vec![] };
    assert_eq!(host.run(request.clone()).await, Response::Done);

    let samba = fs::read_to_string(host.path("samba/zendiloos.conf")).unwrap();
    assert!(samba.contains("[photos]\n   path = /volume1/photos\n   valid users = @family\n"), "{samba}");
    let exports = fs::read_to_string(host.path("exports.d/zendiloos.exports")).unwrap();
    assert!(
        exports.contains(
            "/volume1/photos 192.168.1.0/24(ro,sync,no_subtree_check,all_squash,anonuid=995,anongid=995,"
        ),
        "{exports}"
    );
    assert!(samba.contains("   force user = root\n   force group = zendiloos-files\n"), "{samba}");
    assert_eq!(
        host.calls(),
        [
            "testparm --suppress-prompt /samba/zendiloos.check",
            "systemctl enable --no-reload --now smbd.service",
            // The first announcement over Bonjour: smbd makes it as it starts.
            "systemctl try-restart smbd.service",
            "systemctl enable --no-reload --now wsdd2.service",
            "systemctl enable --no-reload --now avahi-daemon.socket avahi-daemon.service",
            "systemctl enable --no-reload --now nfs-server.service",
            "systemctl reload-or-restart nfs-server.service",
            // Reloaded once if a link changed.
            "systemctl show --property=NeedDaemonReload --value smbd.service",
        ]
    );
    let first = host.calls().len();
    assert_eq!(host.run(request).await, Response::Done);
    assert!(
        host.calls()[first..].contains(&"systemctl try-reload-or-restart smbd.service".to_owned()),
        "the same announcements again: only a reload, open connections stay"
    );
}

fn services(on: bool) -> FileServices {
    FileServices {
        smb: on,
        nfs: on,
        workgroup: Workgroup::parse("WORKGROUP").unwrap(),
        smb_min_protocol: SmbProtocol::Smb2_1,
        bonjour: on,
        ..base_services()
    }
}

#[tokio::test]
async fn servers_that_are_off_stop_and_stay_off_with_their_configuration_kept() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let nfs = vec![NfsExport {
        name: share("photos"),
        volume: VolumeNumber::new(1).unwrap(),
        clients: vec![NfsClient {
            network: NfsHost::parse("192.168.1.0/24").unwrap(),
            write: false,
            asynchronous: false,
            insecure: false,
        }],
    }];

    let request =
        Request::ApplySharing { smb: vec![], nfs, services: services(false), homes: None, close: vec![] };
    assert_eq!(host.run(request).await, Response::Done);

    let samba = fs::read_to_string(host.path("samba/zendiloos.conf")).unwrap();
    assert!(samba.contains("workgroup = WORKGROUP\nserver min protocol = SMB2_10\n"), "{samba}");
    assert!(
        fs::read_to_string(host.path("exports.d/zendiloos.exports")).unwrap().contains("/volume1/photos")
    );
    assert_eq!(
        host.calls(),
        [
            "testparm --suppress-prompt /samba/zendiloos.check",
            "systemctl disable --no-reload --now smbd.service",
            "systemctl disable --no-reload --now wsdd2.service",
            "systemctl disable --no-reload --now avahi-daemon.service avahi-daemon.socket",
            "systemctl disable --no-reload --now nfs-server.service",
            "systemctl show --property=NeedDaemonReload --value smbd.service",
        ]
    );
}

#[tokio::test]
async fn each_agent_serves_only_its_role() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    for request in [
        Request::PrepareShare { volume: VolumeNumber::new(1).unwrap(), name: share("photos") },
        Request::MigrateShare { volume: VolumeNumber::new(1).unwrap(), name: share("photos") },
    ] {
        assert_refused(host.run(request).await, ErrorCode::Forbidden, "not for the System agent");
    }
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
    let rollback = |share_name: &str, pool: &str| Request::RollbackSnapshot {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        share: share(share_name),
        label: label(),
    };
    assert_eq!(host.run(rollback("photos", "tank")).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), "zfs rollback -r tank/photos@zdl-2026-11-05_19.40.01");
    assert_refused(host.run(rollback("nothing", "tank")).await, ErrorCode::Forbidden, "no share");
    assert_refused(host.run(rollback("photos", "foreign")).await, ErrorCode::Forbidden, "not managed");

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

#[tokio::test]
async fn share_properties_go_to_the_share_dataset_only() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    let set = |share_name: &str, pool: &str, quota| Request::SetShareProperties {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        share: share(share_name),
        quota_bytes: quota,
        compression: ShareCompression::Zstd,
        record_size: ShareRecordSize::M1,
    };

    assert_eq!(host.run(set("photos", "tank", Some(5 << 30))).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        "zfs set refquota=5368709120 compression=zstd recordsize=1M tank/photos"
    );
    assert_eq!(host.run(set("photos", "tank", None)).await, Response::Done);
    assert!(host.calls().last().unwrap().starts_with("zfs set refquota=none "));
    assert_refused(host.run(set("nothing", "tank", None)).await, ErrorCode::Forbidden, "no share");
    assert_refused(host.run(set("photos", "foreign", None)).await, ErrorCode::Forbidden, "not managed");

    assert!(matches!(host.run(Request::ShareUsage {}).await, Response::ShareUsage { .. }));
}

#[tokio::test]
async fn docker_moves_onto_a_pool_zendiloos_made() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    fs::write(host.path("etc-docker/daemon.json"), r#"{"log-driver": "local"}"#).unwrap();
    let enable = |pool: &str| Request::EnableDocker {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
    };

    assert_eq!(host.run(enable("tank")).await, Response::Done);

    let calls = host.calls();
    assert!(calls.contains(&"zfs create -o mountpoint=legacy tank/zendiloos.docker".to_owned()), "{calls:?}");
    assert!(calls.contains(&"systemctl start volume1-\\x40docker.mount".to_owned()), "{calls:?}");
    assert_eq!(calls.last().unwrap(), "systemctl restart docker");
    let unit = fs::read_to_string(host.path("units/volume1-\\x40docker.mount")).unwrap();
    assert!(unit.contains("What=tank/zendiloos.docker\nWhere=/volume1/@docker\n"), "{unit}");
    let drop_in = fs::read_to_string(host.path("units/docker.service.d/zendiloos.conf")).unwrap();
    assert!(drop_in.contains("RequiresMountsFor=/volume1/@docker"), "{drop_in}");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(host.path("etc-docker/daemon.json")).unwrap()).unwrap();
    assert_eq!(config["data-root"], "/volume1/@docker");
    assert_eq!(config["storage-driver"], "overlay2");
    assert_eq!(config["log-driver"], "local", "other settings stay");
    let Response::DockerState { installed, data_root, .. } = host.run(Request::DockerState {}).await else {
        panic!("no state")
    };
    assert!(installed);
    assert_eq!(data_root.as_deref(), Some("/volume1/@docker"));

    assert_refused(host.run(enable("foreign")).await, ErrorCode::Forbidden, "not managed");
}

#[tokio::test]
async fn dockers_data_moves_only_between_pools_zendiloos_made() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    fs::write(host.path("etc-docker/daemon.json"), r#"{"data-root": "/volume1/@docker"}"#).unwrap();
    let move_to = |to: &str, volume: u16| Request::MoveDocker {
        from_pool: PoolName::parse("tank").unwrap(),
        from_volume: VolumeNumber::new(1).unwrap(),
        to_pool: PoolName::parse(to).unwrap(),
        to_volume: VolumeNumber::new(volume).unwrap(),
    };

    assert_refused(host.run(move_to("tank", 1)).await, ErrorCode::Forbidden, "already");
    assert_refused(host.run(move_to("foreign", 2)).await, ErrorCode::Forbidden, "not managed");
    assert!(!host.calls().iter().any(|call| call.contains("stop docker")), "Docker kept running");
}

/// SMB on, NFS off, everything else as a fresh install has it.
fn base_services() -> FileServices {
    FileServices {
        smb: true,
        nfs: false,
        workgroup: Workgroup::parse("WORKGROUP").unwrap(),
        smb_min_protocol: SmbProtocol::Smb2_1,
        smb_encryption: zendiloos_agent_proto::SmbEncryption::Default,
        smb_signing: zendiloos_agent_proto::SmbSigning::Default,
        hide_unreadable: true,
        multichannel: true,
        oplocks: true,
        leases: true,
        durable_handles: false,
        mac_compat: true,
        follow_symlinks: true,
        veto_files: None,
        delete_veto_files: false,
        wsdd: true,
        bonjour: true,
    }
}

#[tokio::test]
async fn a_disk_used_as_swap_is_never_erased() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::write(
        host.path("swaps"),
        format!(
            "Filename\tType\tSize\tUsed\tPriority\n{}\tpartition\t8388604\t0\t-2\n",
            host.path("dev/sda").display()
        ),
    )
    .unwrap();
    assert_refused(
        host.run(create("swapped", &["ata-FREE-1", "ata-FREE-2"], 3)).await,
        ErrorCode::Forbidden,
        "swap",
    );
    assert!(!host.calls().iter().any(|call| call.starts_with("sgdisk")), "nothing erased");
}

#[tokio::test]
async fn docker_gets_a_pool_only_when_installed_and_not_on_another_already() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    fs::write(host.path("etc-docker/daemon.json"), r#"{"data-root": "/volume2/@docker"}"#).unwrap();
    let enable = Request::EnableDocker {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
    };

    assert_refused(host.run(enable.clone()).await, ErrorCode::Forbidden, "another pool already");
    fs::remove_file(host.path("dockerd")).unwrap();
    assert_refused(host.run(enable).await, ErrorCode::Forbidden, "not installed");
    assert!(!host.calls().iter().any(|call| call.contains("restart docker")));
}

#[tokio::test]
async fn an_apps_account_goes_with_its_group_and_nobody_elses_does() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::write(
        host.path("passwd"),
        "root:x:0:0::/root:/bin/bash\napp-kuma:x:2001:2001:zendiloOS app:/nonexistent:/usr/sbin/nologin\n\
         app-paperless:x:2003:990::/nonexistent:/usr/sbin/nologin\n",
    )
    .unwrap();
    fs::write(host.path("group"), "root:x:0:\nzendiloos-users:x:990:\napp-kuma:x:2001:\n").unwrap();
    let delete = |app: &str| Request::DeleteAppAccount { app: AppName::parse(app).unwrap() };

    assert_eq!(host.run(delete("kuma")).await, Response::Done);
    let calls = host.calls();
    assert!(calls.contains(&"userdel -- app-kuma".to_owned()), "{calls:?}");
    assert!(calls.contains(&"groupdel -- app-kuma".to_owned()), "the group too: {calls:?}");
    assert_refused(host.run(delete("paperless")).await, ErrorCode::Forbidden, "not managed");
    assert_eq!(host.run(delete("gone")).await, Response::Done, "nothing to delete");
}

#[tokio::test]
async fn accounts_need_the_users_group() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    fs::write(host.path("passwd"), "root:x:0:0::/root:/bin/bash\n").unwrap();
    fs::write(host.path("group"), "root:x:0:\n").unwrap();

    let user = Request::CreateUser { name: zendiloos_agent_proto::UserName::parse("anna").unwrap() };
    assert_refused(host.run(user).await, ErrorCode::Failed, "zendiloos-users is missing");
    assert!(!host.calls().iter().any(|call| call.starts_with("useradd")), "nobody made");
}

/// zfs and systemctl for two pools zendiloOS made, `tank` (volume 1, with
/// Docker's data and an app's) and `cold` (volume 2): a file `receive-fails`
/// makes the copy fail, `start-fails` makes Docker not start.
fn two_pools(host: &Host) {
    let write = |name: &str, body: String| {
        let path = host.path(name);
        let log = host.path("calls.log");
        fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    write(
        "zfs",
        format!(
            "case \"$*\" in\n\
             'get -H -o value zendiloos:volume tank') echo 1 ;;\n\
             'get -H -o value zendiloos:volume cold') echo 2 ;;\n\
             'list -H -o name -d 1 cold') echo cold ;;\n\
             'list -H -o name -d 1 tank') printf 'tank\\ntank/zendiloos.docker\\ntank/zendiloos.homes\\n' ;;\n\
             'list -H -o name -d 2 tank') printf 'tank\\ntank/zendiloos.apps\\ntank/zendiloos.apps/kuma\\n' ;;\n\
             send*) echo stream ;;\n\
             receive*) cat > /dev/null; if [ -f {fails} ]; then echo 'out of space' >&2; exit 1; fi ;;\n\
             *) echo - ;;\n\
             esac",
            fails = host.path("receive-fails").display()
        ),
    );
    write(
        "systemctl",
        format!(
            "[ -f {fails} ] && [ \"$1 $2\" = 'start docker.service' ] && {{ echo 'docker failed' >&2; exit 1; }}\nexit 0",
            fails = host.path("start-fails").display()
        ),
    );
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    fs::write(
        host.path("etc-docker/daemon.json"),
        r#"{"data-root": "/volume1/@docker", "log-driver": "local"}"#,
    )
    .unwrap();
    fs::write(host.path("units/volume1-\\x40docker.mount"), "[Mount]\n").unwrap();
}

fn move_docker() -> Request {
    Request::MoveDocker {
        from_pool: PoolName::parse("tank").unwrap(),
        from_volume: VolumeNumber::new(1).unwrap(),
        to_pool: PoolName::parse("cold").unwrap(),
        to_volume: VolumeNumber::new(2).unwrap(),
    }
}

fn data_root(host: &Host) -> String {
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(host.path("etc-docker/daemon.json")).unwrap()).unwrap();
    config["data-root"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn dockers_data_is_copied_switched_to_and_the_old_destroyed() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);

    assert_eq!(host.run(move_docker()).await, Response::Done);

    let calls = host.calls();
    let at = |call: &str| calls.iter().position(|c| c == call).unwrap_or_else(|| panic!("{call}: {calls:?}"));
    assert!(at("systemctl stop docker.service") < at("zfs snapshot tank/zendiloos.docker@zendiloos-move"));
    assert!(at("zfs send -R tank/zendiloos.docker@zendiloos-move") > at("systemctl stop docker.service"));
    at("zfs receive -u -o mountpoint=legacy cold/zendiloos.docker");
    assert!(
        at("systemctl start docker.service") < at("zfs destroy -r tank/zendiloos.docker"),
        "the old goes last"
    );
    assert_eq!(data_root(&host), "/volume2/@docker");
    assert!(host.path("units/volume2-\\x40docker.mount").exists());
    assert!(!host.path("units/volume1-\\x40docker.mount").exists());
}

#[tokio::test]
async fn a_failed_copy_leaves_docker_on_its_data_running() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);
    fs::write(host.path("receive-fails"), "").unwrap();

    assert_refused(host.run(move_docker()).await, ErrorCode::Failed, "out of space");

    let calls = host.calls();
    assert!(
        calls.contains(&"zfs destroy -r cold/zendiloos.docker".to_owned()),
        "the half copy goes: {calls:?}"
    );
    assert_eq!(calls.last().map(String::as_str), Some("systemctl start docker.service"), "{calls:?}");
    assert_eq!(data_root(&host), "/volume1/@docker");
    assert!(!calls.contains(&"zfs destroy -r tank/zendiloos.docker".to_owned()));
}

#[tokio::test]
async fn a_failed_switch_points_docker_back_at_its_data() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);
    fs::write(host.path("start-fails"), "").unwrap();

    assert_refused(host.run(move_docker()).await, ErrorCode::Failed, "docker failed");

    assert_eq!(data_root(&host), "/volume1/@docker", "pointed back");
    let calls = host.calls();
    assert!(calls.contains(&"zfs destroy -r cold/zendiloos.docker".to_owned()), "{calls:?}");
    assert!(!calls.contains(&"zfs destroy -r tank/zendiloos.docker".to_owned()), "the data stays");
    assert!(!host.path("units/volume2-\\x40docker.mount").exists());
}

#[tokio::test]
async fn docker_moves_only_where_its_data_is_said_to_be() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);
    fs::write(host.path("etc-docker/daemon.json"), r#"{"data-root": "/var/lib/docker"}"#).unwrap();
    assert_refused(host.run(move_docker()).await, ErrorCode::Forbidden, "not on tank");

    fs::write(host.path("etc-docker/daemon.json"), r#"{"data-root": "/volume1/@docker"}"#).unwrap();
    let back = Request::MoveDocker {
        from_pool: PoolName::parse("cold").unwrap(),
        from_volume: VolumeNumber::new(2).unwrap(),
        to_pool: PoolName::parse("tank").unwrap(),
        to_volume: VolumeNumber::new(1).unwrap(),
    };
    assert_refused(host.run(back).await, ErrorCode::Forbidden, "not on cold");
    fs::remove_file(host.path("dockerd")).unwrap();
    assert_refused(host.run(move_docker()).await, ErrorCode::Forbidden, "not installed");
    assert!(!host.calls().iter().any(|call| call.contains("stop docker")), "Docker kept running");
}

#[tokio::test]
async fn personal_folders_get_a_dataset_and_quotas_per_user() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);
    let homes = |pool: &str, volume: u16| Request::PrepareHomes {
        pool: PoolName::parse(pool).unwrap(),
        volume: VolumeNumber::new(volume).unwrap(),
    };

    assert_eq!(host.run(homes("tank", 1)).await, Response::Done);
    assert_eq!(host.run(homes("cold", 2)).await, Response::Done);
    let calls = host.calls();
    assert!(
        !calls.iter().any(|call| call.contains("create") && call.contains("tank/zendiloos.homes")),
        "kept"
    );
    assert!(
        calls.iter().any(|call| call.contains("create") && call.ends_with("cold/zendiloos.homes")),
        "{calls:?}"
    );
    let unit = fs::read_to_string(host.path("units/volume2-\\x40homes.mount")).unwrap();
    assert!(unit.contains("What=cold/zendiloos.homes\nWhere=/volume2/@homes\n"), "{unit}");
    // Prepared again, its unit is the same: systemd is not reloaded for it.
    let reloads = || host.calls().iter().filter(|call| *call == "systemctl daemon-reload").count();
    let before = reloads();
    assert_eq!(host.run(homes("cold", 2)).await, Response::Done);
    assert_eq!(reloads(), before, "{:?}", host.calls());
    assert_eq!(host.calls().last().unwrap(), "systemctl start volume2-\\x40homes.mount");
    assert_refused(host.run(homes("tank", 2)).await, ErrorCode::Forbidden, "not managed");

    let quota = |bytes| Request::SetHomeQuota {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        user: zendiloos_agent_proto::UserName::parse("anna").unwrap(),
        quota_bytes: bytes,
    };
    assert_eq!(host.run(quota(Some(1 << 30))).await, Response::Done);
    assert_eq!(host.run(quota(None)).await, Response::Done);
    let calls = host.calls();
    assert!(
        calls.contains(&"zfs set userquota@anna=1073741824 tank/zendiloos.homes".to_owned()),
        "{calls:?}"
    );
    assert!(calls.contains(&"zfs set userquota@anna=none tank/zendiloos.homes".to_owned()));
}

#[tokio::test]
async fn an_apps_dataset_goes_with_the_app() {
    let host = host(Erasable::Policy(ErasePolicy::Unused));
    two_pools(&host);
    fs::write(host.path("units/volume1-\\x40apps-kuma.mount"), "[Mount]\n").unwrap();
    let remove = |app: &str| Request::RemoveApp {
        app: AppName::parse(app).unwrap(),
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
    };

    // Never installed as far as Compose knows: only its dataset goes.
    assert_eq!(host.run(remove("kuma")).await, Response::Done);
    let calls = host.calls();
    assert!(
        calls.contains(&"systemctl disable --no-reload --now volume1-\\x40apps-kuma.mount".to_owned()),
        "{calls:?}"
    );
    assert!(calls.contains(&"zfs destroy -r tank/zendiloos.apps/kuma".to_owned()));
    assert_eq!(host.run(remove("other")).await, Response::Done, "no dataset: nothing to destroy");
    assert!(!host.calls().contains(&"zfs destroy -r tank/zendiloos.apps/other".to_owned()));
}

/// Replaces the fake program `name` with one that records its arguments
/// and then runs `body`.
fn program(host: &Host, name: &str, body: &str) {
    let path = host.path(name);
    let log = host.path("calls.log");
    fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// `zpool`: tank imported; `zpool import -d …` lists media (id 42) and
/// another media (id 43); importing exits with `status`, saying `said`.
fn importing(host: &Host, status: u8, said: &str) {
    program(
        host,
        "zpool",
        &format!(
            "case \"$1\" in\n\
             list) echo tank ;;\n\
             export) ;;\n\
             import) if [ \"$#\" -eq 3 ]; then printf '   pool: media\\n     id: 42\\n  state: ONLINE\\n config:\\n\\n\\tmedia ONLINE\\n\\n   pool: media\\n     id: 43\\n  state: ONLINE\\n'; else echo \"{said}\" >&2; exit {status}; fi ;;\n\
             esac"
        ),
    );
}

/// `zfs`: volume 1 in use; the imported media says `volume`.
fn media_says(host: &Host, volume: &str) {
    program(
        host,
        "zfs",
        &format!(
            "case \"$*\" in\n\
             *'-s local -d 0'*) echo 1 ;;\n\
             'get -H -o value zendiloos:volume media') echo {volume} ;;\n\
             *) ;;\n\
             esac"
        ),
    );
}

fn import(name: &str, id: u64, volume: u16, adopt: bool, force: bool) -> Request {
    Request::ImportPool {
        name: PoolName::parse(name).unwrap(),
        id,
        volume: VolumeNumber::new(volume).unwrap(),
        adopt,
        force,
    }
}

#[tokio::test]
async fn lists_the_pools_on_the_disks_and_none_when_zfs_finds_none() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    importing(&host, 0, "");
    let Response::ImportablePools { zpool_import } = host.run(Request::ImportablePools {}).await else {
        panic!("not listed")
    };
    assert!(zpool_import.contains("pool: media\n     id: 42"), "{zpool_import}");
    assert_eq!(host.calls(), ["zpool import -d /dev/disk/by-id"], "by-id, as pools are made");

    program(&host, "zpool", "echo 'no pools available to import' >&2; exit 1");
    assert_eq!(
        host.run(Request::ImportablePools {}).await,
        Response::ImportablePools { zpool_import: String::new() }
    );
    program(&host, "zpool", "echo 'cannot open /dev/zfs' >&2; exit 1");
    assert_refused(host.run(Request::ImportablePools {}).await, ErrorCode::Failed, "cannot open");
}

#[tokio::test]
async fn imports_a_pool_by_its_identifier_unmounted_as_the_volume_it_says() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    importing(&host, 0, "");
    media_says(&host, "2");
    assert_eq!(host.run(import("media", 42, 2, false, false)).await, Response::Done);
    assert_eq!(
        host.calls(),
        [
            "zpool list -H -o name",
            "zfs get -H -o value -s local -d 0 zendiloos:volume",
            "zpool import -d /dev/disk/by-id",
            "zpool import -N -d /dev/disk/by-id 42",
            "zfs get -H -o value zendiloos:volume media",
        ]
    );
    assert!(!host.path("units/volume2.mount").exists(), "mounted by its own request");
}

#[tokio::test]
async fn imports_neither_a_name_nor_a_volume_taken_nor_a_pool_not_listed() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    importing(&host, 0, "");
    media_says(&host, "2");
    assert_refused(host.run(import("tank", 42, 2, false, false)).await, ErrorCode::Conflict, "");
    assert_refused(host.run(import("media", 42, 1, false, false)).await, ErrorCode::Conflict, "");
    // A unit of volume 3 mounts another pool.
    fs::write(host.path("units/volume3.mount"), "[Mount]\nWhat=cold\n").unwrap();
    assert_refused(host.run(import("media", 42, 3, false, false)).await, ErrorCode::Conflict, "");
    assert_refused(
        host.run(import("media", 44, 2, false, false)).await,
        ErrorCode::Forbidden,
        "no pool media",
    );
    assert_refused(host.run(import("cold", 42, 2, false, false)).await, ErrorCode::Forbidden, "no pool cold");
    assert!(!host.calls().iter().any(|call| call.starts_with("zpool import -N")), "nothing imported");
}

#[tokio::test]
async fn a_pool_another_machine_used_last_is_imported_only_by_force() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    importing(&host, 1, "cannot import 'media': pool was previously in use from another system.");
    media_says(&host, "2");
    let refused = host.run(import("media", 42, 2, false, false)).await;
    assert_refused(
        refused,
        ErrorCode::Forbidden,
        "ZFS does not import media: cannot import 'media': pool was previously in use from another system.",
    );
    importing(&host, 0, "");
    assert_eq!(host.run(import("media", 43, 2, false, true)).await, Response::Done);
    assert!(
        host.calls().contains(&"zpool import -N -d /dev/disk/by-id -f 43".to_owned()),
        "{:#?}",
        host.calls()
    );
}

#[tokio::test]
async fn a_pool_that_is_another_volume_or_not_zendiloos_s_is_exported_again() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    importing(&host, 0, "");
    media_says(&host, "3");
    assert_refused(
        host.run(import("media", 42, 2, false, false)).await,
        ErrorCode::Forbidden,
        "media is volume 3, not 2",
    );
    assert_eq!(host.calls().last().unwrap(), "zpool export media");

    media_says(&host, "-");
    let foreign = host.run(import("media", 42, 2, true, false)).await;
    assert_refused(foreign, ErrorCode::Forbidden, "not made by zendiloOS");
    assert_eq!(host.calls().last().unwrap(), "zpool export media");

    // Adopted: its volume number becomes the one given.
    media_says(&host, "3");
    assert_eq!(host.run(import("media", 42, 2, true, false)).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), "zfs set zendiloos:volume=2 media");
}

/// `zfs`: tank (volume 1) holds photos and the datasets zendiloos keeps.
fn tank_with_everything(host: &Host) {
    program(
        host,
        "zfs",
        "case \"$*\" in\n\
         *'-o name -d 1 tank') printf 'tank\\ntank/photos\\ntank/zendiloos.homes\\ntank/zendiloos.docker\\ntank/zendiloos.vms\\n' ;;\n\
         *' tank') echo 1 ;;\n\
         *) echo - ;;\n\
         esac",
    );
}

fn mount_volume(volume: u16, shares: &[&str]) -> Request {
    Request::MountVolume {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(volume).unwrap(),
        shares: shares.iter().map(|name| share(name)).collect(),
    }
}

#[tokio::test]
async fn a_volumes_mounts_are_made_as_creating_them_writes_them_and_once() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    tank_with_everything(&host);
    fs::create_dir_all(host.path("etc-docker")).unwrap();
    fs::write(host.path("etc-docker/daemon.json"), "{\"data-root\": \"/volume1/@docker\"}\n").unwrap();

    let made = host.run(mount_volume(1, &["photos", "gone"])).await;

    assert_eq!(made, Response::VolumeMounted { missing: vec![share("gone")], changed: true });
    let units = [
        "volume1.mount",
        "volume1-photos.mount",
        "volume1-\\x40homes.mount",
        "volume1-\\x40docker.mount",
        "volume1-\\x40vms.mount",
    ];
    let mut expected = vec![
        "zfs get -H -o value zendiloos:volume tank".to_owned(),
        "zfs list -H -o name -d 1 tank".to_owned(),
        format!("systemctl enable --no-reload {}", units.join(" ")),
        "systemctl daemon-reload".to_owned(),
    ];
    expected.extend(units.iter().map(|unit| format!("systemctl start {unit}")));
    expected.push("systemctl start docker.service".to_owned());
    assert_eq!(host.calls(), expected, "one reload; Docker starts on its data mounted anew");
    let read = |unit: &str| fs::read_to_string(host.path("units").join(unit)).unwrap();
    assert!(read("volume1.mount").contains("What=tank\nWhere=/volume1\nType=zfs\n"));
    assert!(read("volume1-photos.mount").contains("What=tank/photos\nWhere=/volume1/photos\n"));
    assert!(read("volume1-\\x40homes.mount").contains("What=tank/zendiloos.homes\nWhere=/volume1/@homes\n"));
    assert!(read("volume1-\\x40docker.mount").contains("Before=docker.service\n"));
    assert!(read("volume1-\\x40vms.mount").contains("What=tank/zendiloos.vms\nWhere=/volume1/@vms\n"));
    assert!(!host.path("units/volume1-gone.mount").exists());
    let libvirt = fs::read_to_string(host.path("units/libvirtd.service.d/zendiloos.conf")).unwrap();
    assert!(libvirt.contains("RequiresMountsFor=/volume1/@vms\n"), "{libvirt}");
    let docker = fs::read_to_string(host.path("units/docker.service.d/zendiloos.conf")).unwrap();
    assert!(docker.contains("RequiresMountsFor=/volume1/@docker\n"), "{docker}");

    // Again: nothing written, systemd not reloaded, Docker left alone.
    fs::remove_file(host.path("calls.log")).unwrap();
    let again = host.run(mount_volume(1, &["photos"])).await;
    assert_eq!(again, Response::VolumeMounted { missing: Vec::new(), changed: false });
    let calls = host.calls();
    assert!(
        !calls.iter().any(|call| call == "systemctl daemon-reload" || call.contains("docker.service")),
        "{calls:#?}"
    );
    assert!(calls.contains(&"systemctl show --property=NeedDaemonReload --value volume1.mount".to_owned()));
}

#[tokio::test]
async fn a_share_made_and_its_mount_made_again_are_the_same_unit() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    let created = Request::CreateShare {
        pool: PoolName::parse("tank").unwrap(),
        volume: VolumeNumber::new(1).unwrap(),
        name: share("media"),
    };
    assert_eq!(host.run(created).await, Response::Done);
    let written = fs::read_to_string(host.path("units/volume1-media.mount")).unwrap();
    program(
        &host,
        "zfs",
        "case \"$*\" in\n*'-o name -d 1 tank') printf 'tank\\ntank/media\\n' ;;\n*' tank') echo 1 ;;\n*) echo - ;;\nesac",
    );
    fs::remove_file(host.path("units/volume1.mount")).unwrap();
    let mounted = host.run(mount_volume(1, &["media"])).await;
    assert_eq!(
        mounted,
        Response::VolumeMounted { missing: Vec::new(), changed: true },
        "the volume's unit was gone"
    );
    assert_eq!(
        fs::read_to_string(host.path("units/volume1-media.mount")).unwrap(),
        written,
        "not written anew"
    );
}

#[tokio::test]
async fn only_a_pool_zendiloos_made_gets_its_mounts_and_a_mount_that_fails_is_told() {
    let host = host(Erasable::Policy(ErasePolicy::None));
    tank_with_everything(&host);
    assert_refused(host.run(mount_volume(2, &[])).await, ErrorCode::Forbidden, "not managed");
    assert!(!host.path("units/volume2.mount").exists());
    program(
        &host,
        "systemctl",
        "[ \"$1 $2\" = 'start volume1-photos.mount' ] && { echo 'mount failed' >&2; exit 1; }\nexit 0",
    );
    let failed = host.run(mount_volume(1, &["photos"])).await;
    assert_refused(failed, ErrorCode::Failed, "cannot mount volume1-photos.mount");
    assert!(
        host.calls().contains(&"systemctl start volume1-\\x40vms.mount".to_owned()),
        "the others are mounted"
    );
}
