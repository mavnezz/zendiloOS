//! Virtual machines against a fake host: `virsh`, `zfs`, `systemctl`,
//! `systemd-run` and `busctl` (systemd-networkd's view of the host's links)
//! replaced by scripts that record their arguments and keep what they made
//! as files, the units, `/proc`, `/sys`, the volumes and the consoles'
//! folder in a temporary directory.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, HostPaths, Operations, Paths, VmPaths};
use zendiloos_agent_proto::{
    ErrorCode, InterfaceName, MacAddress, PoolName, Request, Response, Role, ShareLocation, ShareName,
    SharePath, VM_IMAGE_UPLOAD_MAX, VmAction, VmBoot, VmCpus, VmDevices, VmDisk, VmDiskBus, VmDiskSize,
    VmDiskSlot, VmDrive, VmDriveSlot, VmFirmware, VmId, VmImageFile, VmImageId, VmIsoSource, VmMemory,
    VmNetwork, VmNic, VmNicModel, VmNicSlot, VmOs, VmSpec, VmStorage, VolumeNumber,
};

const ID: &str = "0a1b2c3d4e5f";
const IMAGE: &str = "00000000000a";

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

    /// The calls since `from`.
    fn calls_since(&self, from: usize) -> Vec<String> {
        self.calls().split_off(from)
    }

    async fn run(&self, request: Request) -> Response {
        self.operations.execute(&request).await
    }

    /// What libvirt's fake says the domain does.
    fn state(&self) -> String {
        fs::read_to_string(self.path(&format!("state/zendiloos-{ID}"))).unwrap_or_default().trim().to_owned()
    }

    fn zvols(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path("state/zvols"))
            .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    fn put_image(&self, image: &str) {
        fs::create_dir_all(self.path("volume1/@vms/isos")).unwrap();
        fs::write(self.path(&format!("volume1/@vms/isos/{image}.iso")), "iso").unwrap();
    }
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let log = root.join("calls.log");
    fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The pool `tank` (volume 1), zendiloOS's; libvirt and QEMU installed
/// unless `installed` is false. systemd-networkd knows the host's links: a
/// bridge `br0`, a card `enp1s0`, and a bridge `vms0` it does not describe.
fn host(installed: bool) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in ["units", "iommu", "state/zvols", "volume1/media/isos"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    fs::write(root.join("kvm"), "").unwrap();
    fs::write(root.join("cpuinfo"), "processor : 0\nflags : fpu vmx\nprocessor : 1\nflags : fpu vmx\n")
        .unwrap();
    fs::write(root.join("meminfo"), "MemTotal:        2027168 kB\n").unwrap();
    fs::write(root.join("volume1/media/isos/debian.iso"), "iso").unwrap();
    let state = root.join("state");
    // Domains defined are files in state/ holding what they do; zvols are
    // files in state/zvols holding their size.
    let virsh = script(
        &root,
        "virsh",
        &format!(
            "state={state}\nshift 2\n\
             case \"$1\" in\n\
             domstate) test -f \"$state/$2\" && cat \"$state/$2\" ;;\n\
             define) name=$(sed -n 's/.*<name>\\(.*\\)<\\/name>.*/\\1/p' \"$3\"); \
                     uuid=$(sed -n 's/.*<uuid>\\(.*\\)<\\/uuid>.*/\\1/p' \"$3\"); \
                     if [ -f \"$state/$name\" ]; then [ \"$uuid\" = \"$(cat \"$state/uuid.$name\")\" ] || \
                       {{ echo \"error: operation failed: domain '$name' already exists with uuid $(cat \"$state/uuid.$name\")\" >&2; exit 1; }}; \
                     else echo 'shut off' > \"$state/$name\"; echo 8b45cec4-586f-4885-973a-9e24c63b82fe > \"$state/uuid.$name\"; fi ;;\n\
             domuuid) cat \"$state/uuid.$2\" ;;\n\
             undefine) eval last=\\${{$#}}; rm \"$state/$last\" \"$state/uuid.$last\" ;;\n\
             list) printf ' Id   Name                     State\\n----------------------------------------\\n'\n\
                   for f in \"$state\"/zendiloos-*; do [ -e \"$f\" ] && printf ' -    %s   %s\\n' \"$(basename \"$f\")\" \"$(cat \"$f\")\"; done\n\
                   printf ' 3    win10                    running\\n' ;;\n\
             start) grep -q running \"$state/$2\" && {{ echo 'error: Requested operation is not valid: domain is already active' >&2; exit 1; }}; echo running > \"$state/$2\" ;;\n\
             destroy) [ -f \"$state/$2\" ] && echo 'shut off' > \"$state/$2\" ;;\n\
             shutdown) echo 'shut off' > \"$state/$4\" ;;\n\
             domstats) printf \"Domain: 'win10'\\n  cpu.time=1\\n\\nDomain: 'zendiloos-{ID}'\\n  cpu.time=2\\n\" ;;\n\
             esac",
            state = state.display()
        ),
    );
    let zfs = script(
        &root,
        "zfs",
        &format!(
            "state={state}\neval last=\\${{$#}}\n\
             case \"$*\" in\n\
             *'-s local -d 0'*) echo 1 ;;\n\
             'get -H -o value zendiloos:volume tank') echo 1 ;;\n\
             'list -H -o name -d 1 tank') echo tank; if [ -f \"$state/vms\" ]; then echo tank/zendiloos.vms; fi ;;\n\
             'list -H -o name tank/zendiloos.vms') test -f \"$state/vms\" ;;\n\
             'list -H -o name tank/zendiloos.vms/'*) test -f \"$state/zvols/${{last##*/}}\" ;;\n\
             'get -H -p -o value volsize tank/zendiloos.vms/'*) cat \"$state/zvols/${{last##*/}}\" ;;\n\
             'create -s -V '*) echo $(( ${{4%G}} * 1073741824 )) > \"$state/zvols/${{last##*/}}\" ;;\n\
             create*zendiloos.vms) touch \"$state/vms\" ;;\n\
             'set volsize='*) v=${{2#volsize=}}; echo $(( ${{v%G}} * 1073741824 )) > \"$state/zvols/${{last##*/}}\" ;;\n\
             'destroy tank/zendiloos.vms/'*) rm \"$state/zvols/${{last##*/}}\" ;;\n\
             esac",
            state = state.display()
        ),
    );
    let systemd_run = script(
        &root,
        "systemd-run",
        &format!(
            "eval last=\\${{$#}}\n\
             case \"$*\" in\n\
             *copy-vm-iso*) echo '{{\"progress\":{{\"step\":\"copying_iso\",\"percent\":40}}}}'; echo '{{\"done\":{{\"bytes\":3}}}}' ;;\n\
             *receive-vm-image*) cat > {root}/received; n=$(wc -c < {root}/received); echo \"{{\\\"done\\\":{{\\\"bytes\\\":$n}}}}\" ;;\n\
             *'rm --force'*) rm -f \"$last\" ;;\n\
             esac",
            root = root.display()
        ),
    );
    let busctl = script(
        &root,
        "busctl",
        "case \"$*\" in\n\
         *'GetLinkByName s br0') echo 'io 5 \"/org/freedesktop/network1/link/_35\"' ;;\n\
         *'GetLinkByName s enp1s0') echo 'io 2 \"/org/freedesktop/network1/link/_32\"' ;;\n\
         *'GetLinkByName s vms0') echo 'io 6 \"/org/freedesktop/network1/link/_36\"' ;;\n\
         *'DescribeLink i 5') echo '{\"type\":\"s\",\"data\":[\"{\\\"Index\\\":5,\\\"Kind\\\":\\\"bridge\\\",\\\"Type\\\":\\\"bridge\\\"}\"]}' ;;\n\
         *'DescribeLink i 2') echo '{\"type\":\"s\",\"data\":[\"{\\\"Index\\\":2,\\\"Type\\\":\\\"ether\\\"}\"]}' ;;\n\
         *) exit 1 ;;\n\
         esac",
    );
    let mut tools = Tools {
        virsh,
        busctl,
        zfs,
        zpool: script(&root, "zpool", ""),
        systemctl: script(&root, "systemctl", ""),
        systemd_run,
        libvirtd: root.join("libvirtd"),
        qemu: root.join("qemu-system-x86_64"),
        systemd_tmpfiles: PathBuf::from("/usr/bin/systemd-tmpfiles"),
        rm: PathBuf::from("/bin/rm"),
        ..Tools::default()
    };
    if installed {
        fs::write(&tools.libvirtd, "").unwrap();
        fs::write(&tools.qemu, "").unwrap();
    } else {
        tools.virsh = root.join("no-virsh");
    }
    let paths = Paths {
        host: HostPaths {
            units: root.join("units"),
            volumes: root.clone(),
            agent: PathBuf::from("/usr/libexec/zendiloos/zendiloos-agent"),
            share_mounts: false,
            ..HostPaths::default()
        },
        vms: VmPaths {
            kvm: root.join("kvm"),
            cpuinfo: root.join("cpuinfo"),
            meminfo: root.join("meminfo"),
            iommu: root.join("iommu"),
            domains: root.join("domains"),
            consoles: root.join("consoles"),
            tmpfiles: root.join("tmpfiles.d/zendiloos-vms.conf"),
            apparmor: root.join("apparmor/abstractions/libvirt-qemu.d/zendiloos-consoles"),
            zvols: PathBuf::from("/dev/zvol"),
        },
        ..Paths::default()
    };
    let operations =
        Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
    Host { _dir: dir, root, operations }
}

fn pool() -> PoolName {
    PoolName::parse("tank").unwrap()
}

fn volume() -> VolumeNumber {
    VolumeNumber::new(1).unwrap()
}

fn id() -> VmId {
    VmId::parse(ID).unwrap()
}

fn image() -> VmImageId {
    VmImageId::parse(IMAGE).unwrap()
}

fn disk(slot: u8, gib: u32) -> VmDisk {
    VmDisk {
        slot: VmDiskSlot::new(slot).unwrap(),
        size: VmDiskSize::new(gib).unwrap(),
        bus: VmDiskBus::Virtio,
    }
}

fn nic(slot: u8, network: VmNetwork) -> VmNic {
    VmNic {
        slot: VmNicSlot::new(slot).unwrap(),
        model: VmNicModel::Virtio,
        network,
        mac: MacAddress::parse(&format!("52:54:00:00:00:0{slot}")).unwrap(),
    }
}

fn bridge(name: &str) -> VmNetwork {
    VmNetwork::Bridge { bridge: InterfaceName::parse(name).unwrap() }
}

fn drive(slot: u8, image: Option<VmImageId>) -> VmDrive {
    VmDrive { slot: VmDriveSlot::new(slot).unwrap(), image }
}

/// One disk of 1 GiB, a passt card and an empty drive, booted from the
/// disk; or other devices.
fn devices() -> VmDevices {
    VmDevices {
        disks: vec![disk(0, 1)],
        nics: vec![nic(0, VmNetwork::User)],
        drives: vec![drive(0, None)],
        boot: vec![VmBoot::Disk(VmDiskSlot::new(0).unwrap())],
    }
}

fn spec_with(devices: VmDevices, autostart: bool) -> VmSpec {
    VmSpec::new(
        id(),
        VmOs::Linux,
        VmCpus::new(1).unwrap(),
        VmMemory::new(256).unwrap(),
        VmFirmware::Uefi,
        devices,
        autostart,
        pool(),
        volume(),
    )
    .unwrap()
}

fn spec() -> VmSpec {
    spec_with(devices(), false)
}

fn refused(response: Response, code: ErrorCode, says: &str) {
    match response {
        Response::Error { code: got, message } => {
            assert_eq!(got, code, "{message}");
            assert!(message.contains(says), "{message:?} does not say {says:?}");
        }
        other => panic!("not refused: {other:?}"),
    }
}

async fn set_up(host: &Host) {
    assert_eq!(host.run(Request::EnableVms { pool: pool(), volume: volume() }).await, Response::Done);
    // The dataset's mount, which the fake does not make.
    fs::create_dir_all(host.path("volume1/@vms")).unwrap();
}

#[tokio::test]
async fn the_machine_says_what_it_offers() {
    let host = host(true);
    let Response::VmHost { kvm, cpu_virtualization, iommu, installed, cores, memory_bytes, storage } =
        host.run(Request::VmHost {}).await
    else {
        panic!("no facts")
    };
    assert!(kvm && installed && !iommu);
    assert_eq!(cpu_virtualization.as_deref(), Some("vmx"));
    assert_eq!((cores, memory_bytes), (2, 2_027_168 * 1024));
    assert_eq!(storage, None);

    set_up(&host).await;
    let Response::VmHost { storage, .. } = host.run(Request::VmHost {}).await else { panic!("no facts") };
    assert_eq!(storage, Some(VmStorage { pool: pool(), volume: volume() }));
}

#[tokio::test]
async fn the_storage_goes_on_a_pool_and_libvirt_follows_it() {
    let host = host(true);
    set_up(&host).await;
    let calls = host.calls();
    let at = |wanted: &str| {
        calls.iter().position(|call| call == wanted).unwrap_or_else(|| panic!("{wanted}: {calls:?}"))
    };
    at("zfs create -o mountpoint=legacy -o setuid=off -o devices=off -o exec=off tank/zendiloos.vms");
    let enabled = at("systemctl enable --now volume1-\\x40vms.mount");
    assert!(enabled < at("systemctl enable --now virtlogd.socket"));
    assert!(at("systemctl enable --now virtlogd.socket") < at("systemctl enable --now libvirtd.socket"));
    assert!(at("systemctl enable libvirtd.service") < at("systemctl restart libvirtd.service"));
    assert!(
        calls.iter().any(|call| call.starts_with("systemd-run")
            && call.contains("--create /tmpfiles.d/zendiloos-vms.conf"))
    );
    let unit = fs::read_to_string(host.path("units/volume1-\\x40vms.mount")).unwrap();
    assert!(unit.contains("What=tank/zendiloos.vms\nWhere=/volume1/@vms\nType=zfs\n"), "{unit}");
    assert!(unit.contains("Before=libvirtd.service"), "{unit}");
    let drop_in = fs::read_to_string(host.path("units/libvirtd.service.d/zendiloos.conf")).unwrap();
    assert!(drop_in.contains("RequiresMountsFor=/volume1/@vms"), "{drop_in}");
    let tmpfiles = fs::read_to_string(host.path("tmpfiles.d/zendiloos-vms.conf")).unwrap();
    assert!(tmpfiles.contains("consoles 2770 libvirt-qemu root -"), "{tmpfiles}");
    let apparmor =
        fs::read_to_string(host.path("apparmor/abstractions/libvirt-qemu.d/zendiloos-consoles")).unwrap();
    assert!(apparmor.contains("/consoles/*.vnc rw,"), "{apparmor}");
    // No network of libvirt's is ever started or defined.
    assert!(!calls.iter().any(|call| call.contains("net-") || call.contains("nwfilter")), "{calls:?}");

    // A pool with the VMs on it is not destroyed.
    let destroy = host.run(Request::DestroyPool { name: pool(), volume: volume() }).await;
    refused(destroy, ErrorCode::Forbidden, "virtual machines' storage lies on this pool");
    // Nor does the storage go to a second pool.
    fs::write(host.path("units/volume2-\\x40vms.mount"), "[Mount]\nWhat=cold/zendiloos.vms\n").unwrap();
    let again = host.run(Request::EnableVms { pool: pool(), volume: volume() }).await;
    refused(again, ErrorCode::Forbidden, "on another pool");
}

#[tokio::test]
async fn nothing_is_set_up_before_libvirt_is_installed() {
    let host = host(false);
    let refusal = host.run(Request::EnableVms { pool: pool(), volume: volume() }).await;
    refused(refusal, ErrorCode::Forbidden, "not installed");
    let Response::VmStates { virsh_list } = host.run(Request::VmStates {}).await else { panic!("no states") };
    assert!(virsh_list.is_empty());
    let Response::VmStats { domstats } = host.run(Request::VmStats {}).await else { panic!("no stats") };
    assert!(domstats.is_empty());
}

#[tokio::test]
async fn a_vm_gets_its_sparse_zvols_and_a_domain_of_its_own() {
    let host = host(true);
    let define = |spec: VmSpec| Request::DefineVm { spec };
    refused(host.run(define(spec())).await, ErrorCode::Forbidden, "not set up");
    set_up(&host).await;

    let with_image = VmDevices { drives: vec![drive(0, Some(image()))], ..devices() };
    refused(
        host.run(define(spec_with(with_image.clone(), false))).await,
        ErrorCode::Forbidden,
        "not in the library",
    );
    // The host's links, asked of systemd-networkd: a card is no bridge,
    // nor is a link it does not know.
    for other in ["enp1s0", "br7"] {
        let card = VmDevices { nics: vec![nic(0, bridge(other))], ..devices() };
        refused(host.run(define(spec_with(card, false))).await, ErrorCode::Forbidden, "no bridge");
    }
    assert!(!host.calls().iter().any(|call| call.starts_with("zfs create -s")), "nothing made for a refusal");
    let asked = host.calls();
    assert!(
        asked.iter().any(|call| call.ends_with("org.freedesktop.network1.Manager GetLinkByName s enp1s0"))
            && asked.iter().any(
                |call| call.starts_with("busctl --json=short call") && call.ends_with("DescribeLink i 2")
            ),
        "{asked:?}"
    );

    host.put_image(IMAGE);
    let two = VmDevices {
        disks: vec![disk(0, 1), disk(1, 4)],
        nics: vec![nic(0, VmNetwork::User), nic(1, bridge("br0"))],
        ..with_image
    };
    assert_eq!(host.run(define(spec_with(two, true))).await, Response::Done);
    let calls = host.calls();
    for wanted in [
        format!("zfs create -s -V 1G -o volmode=dev tank/zendiloos.vms/{ID}-disk0"),
        format!("zfs create -s -V 4G -o volmode=dev tank/zendiloos.vms/{ID}-disk1"),
        format!("virsh --connect qemu:///system define --validate /domains/zendiloos-{ID}.xml.new"),
    ] {
        assert!(calls.contains(&wanted), "{wanted}: {calls:?}");
    }
    assert_eq!(calls.last().unwrap(), &format!("virsh --connect qemu:///system autostart zendiloos-{ID}"));
    assert_eq!(host.zvols(), [format!("{ID}-disk0"), format!("{ID}-disk1")]);
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(!host.path(&format!("domains/zendiloos-{ID}.xml.new")).exists());
    for part in [
        format!("<source dev='/dev/zvol/tank/zendiloos.vms/{ID}-disk1'/>"),
        format!("<source file='{}/volume1/@vms/isos/{IMAGE}.iso'/>", host.root.display()),
        "<source bridge='br0'/>".into(),
        "<mac address='52:54:00:00:00:01'/>".into(),
        format!("consoles/{ID}.vnc'/>"),
    ] {
        assert!(xml.contains(&part), "{part} missing in {xml}");
    }

    refused(host.run(define(spec())).await, ErrorCode::Conflict, "taken");

    assert_eq!(host.run(Request::SetVmAutostart { id: id(), autostart: false }).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        &format!("virsh --connect qemu:///system autostart --disable zendiloos-{ID}")
    );
}

#[tokio::test]
async fn a_vm_that_is_off_changes_its_hardware_and_disks_only_grow() {
    let host = host(true);
    set_up(&host).await;
    let two = VmDevices { disks: vec![disk(0, 1), disk(1, 4)], ..devices() };
    assert_eq!(host.run(Request::DefineVm { spec: spec_with(two, false) }).await, Response::Done);
    let slot = |n| VmDiskSlot::new(n).unwrap();

    // disk0 grown, disk1 removed, disk2 added, and two cards.
    let changed = VmDevices {
        disks: vec![disk(0, 3), disk(2, 5)],
        nics: vec![nic(0, VmNetwork::User), nic(1, bridge("br0"))],
        boot: vec![VmBoot::Disk(slot(2)), VmBoot::Disk(slot(0))],
        ..devices()
    };
    let from = host.calls().len();
    let update = |devices: VmDevices, remove: Vec<VmDiskSlot>| Request::UpdateVm {
        spec: spec_with(devices, false),
        remove,
    };
    assert_eq!(host.run(update(changed.clone(), vec![slot(1)])).await, Response::Done);
    let calls = host.calls_since(from);
    let at = |wanted: &str| {
        calls.iter().position(|call| call == wanted).unwrap_or_else(|| panic!("{wanted}: {calls:?}"))
    };
    let grown = at(&format!("zfs set volsize=3G tank/zendiloos.vms/{ID}-disk0"));
    let made = at(&format!("zfs create -s -V 5G -o volmode=dev tank/zendiloos.vms/{ID}-disk2"));
    let defined =
        at(&format!("virsh --connect qemu:///system define --validate /domains/zendiloos-{ID}.xml.new"));
    let disabled = at(&format!("virsh --connect qemu:///system autostart --disable zendiloos-{ID}"));
    let removed = at(&format!("zfs destroy tank/zendiloos.vms/{ID}-disk1"));
    assert!(grown < defined && made < defined && defined < disabled && disabled < removed, "{calls:?}");
    assert_eq!(host.zvols(), [format!("{ID}-disk0"), format!("{ID}-disk2")]);
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(
        xml.contains(&format!(
            "{ID}-disk2'/>\n      <target dev='vdc' bus='virtio'/>\n      <boot order='1'/>"
        )),
        "{xml}"
    );

    // A disk never shrinks; nothing is touched then.
    let from = host.calls().len();
    let smaller = VmDevices { disks: vec![disk(0, 2), disk(2, 5)], ..changed.clone() };
    refused(
        host.run(update(smaller, vec![])).await,
        ErrorCode::Forbidden,
        "disk 0 has 3 GiB: a disk only grows",
    );
    assert!(
        !host.calls_since(from).iter().any(|call| call.starts_with("zfs set") || call.contains(" define "))
    );
    refused(
        host.run(update(changed.clone(), vec![slot(0)])).await,
        ErrorCode::Forbidden,
        "both kept and removed",
    );

    // Nor does its hardware change while it runs.
    assert_eq!(host.run(Request::ControlVm { id: id(), action: VmAction::Start }).await, Response::Done);
    let from = host.calls().len();
    refused(host.run(update(changed, vec![])).await, ErrorCode::Forbidden, "not shut off");
    assert!(!host.calls_since(from).iter().any(|call| call.starts_with("zfs ") || call.contains(" define ")));

    let other = VmSpec::new(
        VmId::parse("00000000000f").unwrap(),
        VmOs::Linux,
        VmCpus::new(1).unwrap(),
        VmMemory::new(256).unwrap(),
        VmFirmware::Uefi,
        devices(),
        false,
        pool(),
        volume(),
    )
    .unwrap();
    refused(
        host.run(Request::UpdateVm { spec: other, remove: vec![] }).await,
        ErrorCode::Forbidden,
        "no virtual machine",
    );
}

#[tokio::test]
async fn a_drives_medium_changes_while_the_vm_runs() {
    let host = host(true);
    set_up(&host).await;
    host.put_image(IMAGE);
    assert_eq!(host.run(Request::DefineVm { spec: spec() }).await, Response::Done);
    let first = VmDriveSlot::new(0).unwrap();
    let with =
        |image: Option<VmImageId>| spec_with(VmDevices { drives: vec![drive(0, image)], ..devices() }, false);

    // Off: only the definition changes.
    let from = host.calls().len();
    let insert = Request::ChangeVmMedia { spec: with(Some(image())), drive: first };
    assert_eq!(host.run(insert.clone()).await, Response::Done);
    let calls = host.calls_since(from);
    assert!(calls.iter().any(|call| call.contains(" define --validate ")), "{calls:?}");
    assert!(!calls.iter().any(|call| call.contains("change-media")), "{calls:?}");

    assert_eq!(host.run(Request::ControlVm { id: id(), action: VmAction::Start }).await, Response::Done);
    assert_eq!(host.state(), "running");
    assert_eq!(host.run(insert).await, Response::Done);
    let file = format!("/volume1/@vms/isos/{IMAGE}.iso");
    assert_eq!(
        host.calls().last().unwrap(),
        &format!(
            "virsh --connect qemu:///system change-media zendiloos-{ID} sdi {file} --update --live --force"
        )
    );
    assert_eq!(host.run(Request::ChangeVmMedia { spec: with(None), drive: first }).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        &format!("virsh --connect qemu:///system change-media zendiloos-{ID} sdi --eject --live --force")
    );
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(!xml.contains(&file), "the definition follows: {xml}");

    let missing = Request::ChangeVmMedia { spec: with(None), drive: VmDriveSlot::new(2).unwrap() };
    refused(host.run(missing).await, ErrorCode::Forbidden, "no drive 2");
    let gone = with(Some(VmImageId::parse("00000000000b").unwrap()));
    refused(
        host.run(Request::ChangeVmMedia { spec: gone, drive: first }).await,
        ErrorCode::Forbidden,
        "not in the library",
    );
}

#[tokio::test]
async fn an_iso_is_copied_into_the_library_by_a_unit_that_may_write_only_the_vms_folder() {
    let host = host(true);
    set_up(&host).await;
    let source = VmIsoSource {
        share: ShareLocation { volume: volume(), share: ShareName::parse("media").unwrap() },
        path: SharePath::parse("isos/debian.iso").unwrap(),
    };
    let copy = Request::CopyVmImage { image: image(), volume: volume(), source };
    assert_eq!(host.run(copy).await, Response::VmImageStored { bytes: 3 });
    let call = host.calls().into_iter().find(|call| call.contains("copy-vm-iso")).expect("a unit");
    for part in [
        &format!("--unit=zendiloos-vm-iso-{IMAGE}.service") as &str,
        "--property=ReadWritePaths=/volume1/@vms ",
        "--property=CapabilityBoundingSet=CAP_DAC_READ_SEARCH ",
        "--property=ProtectSystem=strict",
        "--property=PrivateNetwork=yes",
        "/usr/libexec/zendiloos/zendiloos-agent copy-vm-iso ",
    ] {
        assert!(call.contains(part), "{part} missing in {call}");
    }
}

#[tokio::test]
async fn the_library_lists_its_images_and_removes_them_in_a_unit() {
    let host = host(true);
    set_up(&host).await;
    let listed = || async { host.run(Request::VmImages { volume: volume() }).await };
    assert_eq!(listed().await, Response::VmImages { files: vec![] });
    host.put_image(IMAGE);
    let isos = host.path("volume1/@vms/isos");
    fs::write(isos.join("notes.txt"), "x").unwrap();
    fs::write(isos.join(".00000000000c.iso.part"), "x").unwrap();
    symlink(host.path("volume1/media/isos/debian.iso"), isos.join("00000000000b.iso")).unwrap();
    assert_eq!(listed().await, Response::VmImages { files: vec![VmImageFile { image: image(), bytes: 3 }] });

    assert_eq!(host.run(Request::DeleteVmImage { image: image(), volume: volume() }).await, Response::Done);
    let rm = host.calls().into_iter().find(|call| call.contains("rm --force")).expect("removed in a unit");
    assert!(
        rm.contains("--property=ReadWritePaths=/volume1/@vms ")
            && rm.contains("--property=CapabilityBoundingSet= ")
            && rm.ends_with(&format!("-- /volume1/@vms/isos/{IMAGE}.iso")),
        "{rm}"
    );
    assert!(!isos.join(format!("{IMAGE}.iso")).exists());
    // Gone already is no failure.
    assert_eq!(host.run(Request::DeleteVmImage { image: image(), volume: volume() }).await, Response::Done);
    let elsewhere = Request::VmImages { volume: VolumeNumber::new(2).unwrap() };
    refused(host.run(elsewhere).await, ErrorCode::Forbidden, "not 2");
}

#[tokio::test]
async fn an_upload_goes_through_a_pipe_into_a_unit_without_capabilities() {
    let host = host(true);
    set_up(&host).await;
    let open = |bytes| Request::OpenVmImageUpload { image: image(), volume: volume(), bytes };
    let (response, pipe) = host.operations.execute_with_file(&open(5)).await;
    assert_eq!(response, Response::VmImageUpload {});
    let mut pipe = fs::File::from(pipe.expect("the pipe's writing end comes along"));
    let (twice, none) = host.operations.execute_with_file(&open(5)).await;
    refused(twice, ErrorCode::Conflict, "taken");
    assert!(none.is_none());
    pipe.write_all(b"hello").unwrap();
    drop(pipe);
    let finish = Request::FinishVmImageUpload { image: image() };
    assert_eq!(host.run(finish.clone()).await, Response::VmImageStored { bytes: 5 });
    assert_eq!(fs::read(host.path("received")).unwrap(), b"hello");
    let call = host.calls().into_iter().find(|call| call.contains("receive-vm-image")).expect("a unit");
    for part in [
        &format!("--unit=zendiloos-vm-upload-{IMAGE}.service") as &str,
        "--pipe",
        "--property=ReadWritePaths=/volume1/@vms ",
        "--property=CapabilityBoundingSet= ",
        "--property=ProtectSystem=strict",
        "/usr/libexec/zendiloos/zendiloos-agent receive-vm-image ",
    ] {
        assert!(call.contains(part), "{part} missing in {call}");
    }
    refused(host.run(finish).await, ErrorCode::Forbidden, "no upload");

    for bytes in [0, VM_IMAGE_UPLOAD_MAX + 1] {
        let (refusal, pipe) = host.operations.execute_with_file(&open(bytes)).await;
        refused(refusal, ErrorCode::Forbidden, "1 byte to 32 GiB");
        assert!(pipe.is_none());
    }
    // An image of that name is in the library already.
    host.put_image(IMAGE);
    refused(host.operations.execute_with_file(&open(5)).await.0, ErrorCode::Conflict, "taken");
}

#[tokio::test]
async fn vms_are_started_and_stopped_only_by_their_domain() {
    let host = host(true);
    set_up(&host).await;
    let control = |action| Request::ControlVm { id: id(), action };
    refused(host.run(control(VmAction::Start)).await, ErrorCode::Forbidden, "no virtual machine");
    assert_eq!(host.run(Request::DefineVm { spec: spec() }).await, Response::Done);

    for (action, args) in [
        (VmAction::Start, "start"),
        (VmAction::Shutdown, "shutdown --mode acpi"),
        (VmAction::Reboot, "reboot --mode acpi"),
        (VmAction::ForceOff, "destroy"),
        (VmAction::Start, "start"),
    ] {
        assert_eq!(host.run(control(action)).await, Response::Done, "{action:?}");
        assert_eq!(
            host.calls().last().unwrap(),
            &format!("virsh --connect qemu:///system {args} zendiloos-{ID}")
        );
    }
    // libvirt's own words come back.
    refused(host.run(control(VmAction::Start)).await, ErrorCode::Forbidden, "domain is already active");

    let Response::VmStates { virsh_list } = host.run(Request::VmStates {}).await else { panic!("no states") };
    assert!(virsh_list.contains(&format!("zendiloos-{ID}   running")), "{virsh_list}");
    // One look at all, and only zendiloOS's domains in the answer.
    let Response::VmStats { domstats } = host.run(Request::VmStats {}).await else { panic!("no stats") };
    assert_eq!(domstats, format!("Domain: 'zendiloos-{ID}'\n  cpu.time=2\n"));
    assert_eq!(
        host.calls().last().unwrap(),
        "virsh --connect qemu:///system domstats --list-active --nowait --cpu-total --balloon --interface"
    );
}

#[tokio::test]
async fn deleting_a_vm_removes_its_domain_disks_and_xml_but_not_the_library() {
    let host = host(true);
    set_up(&host).await;
    host.put_image(IMAGE);
    let two =
        VmDevices { disks: vec![disk(0, 1), disk(3, 1)], drives: vec![drive(0, Some(image()))], ..devices() };
    assert_eq!(host.run(Request::DefineVm { spec: spec_with(two, false) }).await, Response::Done);

    assert_eq!(
        host.run(Request::DeleteVm { id: id(), pool: pool(), volume: volume() }).await,
        Response::Done
    );
    let calls = host.calls();
    let at = |prefix: &str| {
        calls
            .iter()
            .position(|call| call.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix}: {calls:?}"))
    };
    let undefine = at(&format!(
        "virsh --connect qemu:///system undefine --nvram --managed-save --snapshots-metadata zendiloos-{ID}"
    ));
    assert!(at(&format!("virsh --connect qemu:///system destroy zendiloos-{ID}")) < undefine);
    assert!(undefine < at(&format!("zfs destroy tank/zendiloos.vms/{ID}-disk0")));
    assert!(undefine < at(&format!("zfs destroy tank/zendiloos.vms/{ID}-disk3")));
    assert!(host.zvols().is_empty());
    assert!(!calls.iter().any(|call| call.contains("rm --force")), "the library's images stay");
    assert!(host.path(&format!("volume1/@vms/isos/{IMAGE}.iso")).exists());
    assert!(!host.path(&format!("domains/zendiloos-{ID}.xml")).exists());

    // Gone already is no failure: a VM whose creation failed goes too.
    assert_eq!(
        host.run(Request::DeleteVm { id: id(), pool: pool(), volume: volume() }).await,
        Response::Done
    );
    let foreign = Request::DeleteVm { id: id(), pool: PoolName::parse("cold").unwrap(), volume: volume() };
    refused(host.run(foreign).await, ErrorCode::Forbidden, "not on pool cold");
}

#[tokio::test]
async fn the_console_is_handed_over_connected_and_never_through_a_link() {
    let host = host(true);
    fs::create_dir_all(host.path("consoles")).unwrap();
    let open = || Request::OpenVmConsole { id: id() };
    refused(host.operations.execute_with_file(&open()).await.0, ErrorCode::Forbidden, "not running");

    // Something not QEMU's put a link there: to Docker's socket, say.
    let elsewhere = std::os::unix::net::UnixListener::bind(host.path("docker.sock")).unwrap();
    symlink(host.path("docker.sock"), host.path(&format!("consoles/{ID}.vnc"))).unwrap();
    refused(host.operations.execute_with_file(&open()).await.0, ErrorCode::Forbidden, "no socket");
    drop(elsewhere);
    fs::remove_file(host.path(&format!("consoles/{ID}.vnc"))).unwrap();

    let qemu = std::os::unix::net::UnixListener::bind(host.path(&format!("consoles/{ID}.vnc"))).unwrap();
    let server = std::thread::spawn(move || {
        let (mut client, _) = qemu.accept().unwrap();
        client.write_all(b"RFB 003.008\n").unwrap();
        let mut answer = [0u8; 12];
        client.read_exact(&mut answer).unwrap();
        answer
    });
    let (response, socket) = host.operations.execute_with_file(&open()).await;
    assert_eq!(response, Response::VmConsole {});
    let mut socket = std::os::unix::net::UnixStream::from(socket.expect("the socket comes along"));
    let mut greeting = [0u8; 12];
    socket.read_exact(&mut greeting).unwrap();
    assert_eq!(&greeting, b"RFB 003.008\n");
    socket.write_all(b"RFB 003.008\n").unwrap();
    assert_eq!(&server.join().unwrap(), b"RFB 003.008\n");
}

/// A link systemd-networkd knows but does not describe is taken as it is:
/// libvirt refuses to start a VM on anything but a bridge.
#[tokio::test]
async fn a_bridge_systemd_networkd_does_not_describe_is_taken() {
    let host = host(true);
    set_up(&host).await;
    let card = VmDevices { nics: vec![nic(0, bridge("vms0"))], ..devices() };
    assert_eq!(host.run(Request::DefineVm { spec: spec_with(card, false) }).await, Response::Done);
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(xml.contains("<source bridge='vms0'/>"), "{xml}");
}
