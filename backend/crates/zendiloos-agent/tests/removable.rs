//! Removable drives against a fake host: `lsblk`, `systemd-mount`,
//! `systemd-run` and `umount` replaced by scripts that record their
//! arguments and keep a mount table in a file, as the kernel would.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{AccountFiles, Erasable, ErasePolicy, HostPaths, Operations, Paths};
use zendiloos_agent_proto::{ErrorCode, RemovableId, Request, Response, Role};

const ID: &str = "1A2B-3C4D";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let log = root.join("calls.log");
        let mountinfo = root.join("mountinfo");
        fs::write(&mountinfo, "22 1 0:21 / / rw,relatime - ext4 /dev/vda2 rw\n").unwrap();
        fs::write(root.join("group"), "root:x:0:\nzendiloos-users:x:990:\n").unwrap();
        fs::create_dir_all(root.join("sys-block/sdf1")).unwrap();
        let script = |name: &str, body: &str| -> PathBuf {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let any = script("tool", "");
        let table = mountinfo.display();
        let tools = Tools {
            openssl: "/bin/false".into(),
            nginx: "/bin/false".into(),
            rm: "/bin/false".into(),
            nft: "/bin/false".into(),
            lsblk: script("lsblk", &format!("cat {}", root.join("lsblk.json").display())),
            // Refuses to write a drive while `read-only` exists, as for a
            // write-protected stick.
            systemd_mount: script(
                "systemd-mount",
                &format!(
                    "for a; do device=$place; place=$a; done\n\
                     case \"$*\" in *,rw,*) [ -f {ro} ] && {{ echo 'write-protected' >&2; exit 1; }} ;; esac\n\
                     options=$(printf '%s\\n' \"$@\" | sed -n 's/^--options=//p')\n\
                     mkdir -p \"$place\"\n\
                     echo \"90 22 8:81 / $place $options - vfat $device rw\" >> {table}",
                    ro = root.join("read-only").display()
                ),
            ),
            // `--wait` runs the command, as the transient unit would.
            systemd_run: script(
                "systemd-run",
                "case \"$1\" in --wait) while [ \"${1#--}\" != \"$1\" ]; do shift; done; exec \"$@\" ;; esac",
            ),
            // Busy while `busy` exists, unless lazy.
            umount: script(
                "umount",
                &format!(
                    "for a; do place=$a; done\n\
                     [ \"$1\" != --lazy ] && [ -f {busy} ] && {{ echo \"umount: $place: target is busy.\" >&2; exit 32; }}\n\
                     grep -v \" $place \" {table} > {table}.new; mv {table}.new {table}",
                    busy = root.join("busy").display()
                ),
            ),
            rmdir: script("rmdir", "rmdir \"$@\""),
            systemctl: any.clone(),
            smartctl: any.clone(),
            zpool: any.clone(),
            zfs: any.clone(),
            sgdisk: any.clone(),
            wipefs: any.clone(),
            udevadm: any.clone(),
            testparm: any.clone(),
            exportfs: any.clone(),
            smbcontrol: any.clone(),
            useradd: any.clone(),
            userdel: any.clone(),
            groupadd: any.clone(),
            groupdel: any.clone(),
            gpasswd: any.clone(),
            smbpasswd: any.clone(),
            busctl: any.clone(),
            timedatectl: any.clone(),
            sshd: any.clone(),
            docker: any.clone(),
            setsid: "/usr/bin/setsid".into(),
            dockerd: any,
        };
        let host = HostPaths {
            mountinfo,
            removable: root.join("removable"),
            sys_block: root.join("sys-block"),
            agent: script("agent", ""),
            ..HostPaths::default()
        };
        let accounts = AccountFiles {
            passwd: root.join("passwd"),
            group: root.join("group"),
            high_water: root.join("high-water"),
        };
        let paths = Paths { host, accounts, ..Paths::default() };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
        let host = Self { _dir: dir, root, agent };
        host.plug(&json!([]));
        host
    }

    /// What lsblk lists.
    fn plug(&self, devices: &serde_json::Value) {
        let listing = json!({ "blockdevices": devices }).to_string();
        fs::write(self.root.join("lsblk.json"), listing).unwrap();
    }

    fn stick(&self, mounted: bool) {
        let place = self.place();
        let mount = mounted.then_some(place);
        self.plug(&json!([{
            "kname": "sdf", "type": "disk", "tran": "usb", "rm": true, "ro": false, "fstype": null,
            "uuid": null, "mountpoints": [null],
            "children": [{ "kname": "sdf1", "type": "part", "rm": true, "ro": false, "fstype": "vfat",
                           "uuid": ID, "mountpoints": [mount] }],
        }]));
    }

    fn place(&self) -> String {
        self.root.join("removable").join(ID).display().to_string()
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.lines().filter(|line| !line.starts_with("lsblk ")).map(str::to_owned).collect()
    }

    fn mountinfo(&self) -> String {
        fs::read_to_string(self.root.join("mountinfo")).unwrap()
    }
}

fn id() -> RemovableId {
    RemovableId::parse(ID).unwrap()
}

#[tokio::test]
async fn a_stick_is_mounted_at_its_place_with_safe_options() {
    let host = Host::new();
    host.stick(false);

    let answer = host.agent.execute(&Request::MountRemovable { id: id() }).await;

    assert_eq!(answer, Response::RemovableMounted { read_only: false });
    let place = host.place();
    assert_eq!(
        host.calls(),
        [format!(
            "systemd-mount --no-ask-password --collect --fsck=no --automount=no --type=vfat \
             --options=nosuid,nodev,noexec,noatime,rw,uid=0,gid=990,fmask=0117,dmask=0007,utf8,shortname=mixed,flush \
             --description=zendiloOS: removable drive {ID} -- /dev/sdf1 {place}"
        )]
    );
    // Asked again, it is mounted already.
    host.stick(true);
    assert_eq!(
        host.agent.execute(&Request::MountRemovable { id: id() }).await,
        Response::RemovableMounted { read_only: false }
    );
    assert_eq!(host.calls().len(), 1);
}

#[tokio::test]
async fn a_drive_that_cannot_be_written_is_mounted_read_only() {
    let host = Host::new();
    host.stick(false);
    fs::write(host.root.join("read-only"), "").unwrap();

    let answer = host.agent.execute(&Request::MountRemovable { id: id() }).await;

    assert_eq!(answer, Response::RemovableMounted { read_only: true });
    let calls = host.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls[1].contains(",ro,uid=0"), "{calls:?}");
}

#[tokio::test]
async fn what_the_checks_refuse_is_never_mounted() {
    let host = Host::new();
    // On a SATA disk: no removable drive.
    host.plug(&json!([{
        "kname": "sda", "type": "disk", "tran": "sata", "rm": false, "ro": false, "mountpoints": [null],
        "children": [{ "kname": "sda1", "type": "part", "fstype": "vfat", "uuid": ID, "mountpoints": [null] }],
    }]));

    let answer = host.agent.execute(&Request::MountRemovable { id: id() }).await;

    let Response::Error { code, message } = answer else { panic!("mounted") };
    assert_eq!(code, ErrorCode::Forbidden);
    assert!(message.contains("no USB or removable disk"), "{message}");
    assert_eq!(host.calls(), Vec::<String>::new());
}

#[tokio::test]
async fn ejecting_unmounts_removes_the_place_and_powers_the_disk_off() {
    let host = Host::new();
    host.stick(false);
    host.agent.execute(&Request::MountRemovable { id: id() }).await;
    host.stick(true);
    assert!(host.mountinfo().contains(&host.place()));

    let answer = host.agent.execute(&Request::EjectRemovable { id: id(), power_off: true }).await;

    assert_eq!(answer, Response::Done);
    let place = host.place();
    let calls = host.calls();
    let agent = host.root.join("agent").display().to_string();
    let umount = host.root.join("umount").display().to_string();
    let rmdir = host.root.join("rmdir").display().to_string();
    assert_eq!(
        calls[1..],
        [
            format!("systemd-run --wait --pipe --collect --quiet {umount} -- {place}"),
            format!("umount -- {place}"),
            format!("systemd-run --wait --pipe --collect --quiet {rmdir} -- {place}"),
            format!("rmdir -- {place}"),
            format!(
                "systemd-run --wait --pipe --collect --quiet --description=zendiloOS: power off sdf {agent} \
                 power-off-drive sdf"
            ),
            "agent power-off-drive sdf".to_owned(),
        ]
    );
    assert!(!host.mountinfo().contains(&place));
    assert!(!host.root.join("removable").join(ID).exists());
}

#[tokio::test]
async fn a_drive_in_use_is_not_ejected() {
    let host = Host::new();
    host.stick(false);
    host.agent.execute(&Request::MountRemovable { id: id() }).await;
    host.stick(true);
    fs::write(host.root.join("busy"), "").unwrap();

    let answer = host.agent.execute(&Request::EjectRemovable { id: id(), power_off: true }).await;

    let Response::Error { code, message } = answer else { panic!("ejected") };
    assert_eq!(code, ErrorCode::Conflict);
    assert!(message.contains("still open"), "{message}");
    assert!(host.mountinfo().contains(&host.place()), "still mounted");
    assert!(!host.calls().iter().any(|call| call.contains("power-off-drive")), "never powered off");
}

#[tokio::test]
async fn drives_pulled_out_are_let_go_of_and_stray_places_removed() {
    let host = Host::new();
    let removable = host.root.join("removable");
    // Pulled out while mounted: its device is gone.
    let pulled = removable.join("9F00-0001");
    fs::create_dir_all(&pulled).unwrap();
    let line = format!("91 22 8:97 / {} rw,nosuid - vfat /dev/sdz1 rw\n", pulled.display());
    fs::write(host.root.join("mountinfo"), host.mountinfo() + &line).unwrap();
    // Still there and mounted: stays.
    host.stick(false);
    host.agent.execute(&Request::MountRemovable { id: id() }).await;
    // Empty, from before; and something that is no place of a drive.
    fs::create_dir_all(removable.join("ABCD-0002")).unwrap();
    fs::create_dir_all(removable.join("not a drive")).unwrap();

    let answer = host.agent.execute(&Request::CleanRemovable {}).await;

    let ids = |list: &[&str]| list.iter().map(|id| RemovableId::parse(id).unwrap()).collect();
    assert_eq!(answer, Response::RemovableCleaned { ids: ids(&["9F00-0001", "ABCD-0002"]) });
    let umount = host.root.join("umount").display().to_string();
    assert!(
        host.calls().contains(&format!(
            "systemd-run --wait --pipe --collect --quiet {umount} --lazy -- {}",
            pulled.display()
        )),
        "{:?}",
        host.calls()
    );
    assert!(!pulled.exists());
    assert!(removable.join(ID).exists() && host.mountinfo().contains(&host.place()));
    assert!(removable.join("not a drive").exists(), "left alone");
}
