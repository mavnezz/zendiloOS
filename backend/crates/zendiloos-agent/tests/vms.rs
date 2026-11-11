//! Virtual machines against a fake host: `virsh`, `zfs`, `systemctl` and
//! `systemd-run` replaced by scripts that record their arguments, the
//! units, `/proc`, `/sys` and the consoles' folder in a temporary
//! directory.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, HostPaths, Operations, Paths, VmPaths};
use zendiloos_agent_proto::{
    ErrorCode, InterfaceName, PoolName, Request, Response, Role, ShareLocation, ShareName, SharePath,
    VmAction, VmCpus, VmDiskSize, VmFirmware, VmId, VmIsoSource, VmMemory, VmNetwork, VmOs, VmSpec,
    VmStorage, VolumeNumber,
};

const ID: &str = "0a1b2c3d4e5f";

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

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let log = root.join("calls.log");
    fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The pool `tank` (volume 1), zendiloOS's; libvirt and QEMU installed
/// unless `installed` is false; a bridge `br0` and a card `enp1s0`.
fn host(installed: bool) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in ["units", "net/br0/bridge", "net/enp1s0", "iommu", "state", "volume1/media/isos"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    fs::write(root.join("kvm"), "").unwrap();
    fs::write(root.join("cpuinfo"), "processor : 0\nflags : fpu vmx\nprocessor : 1\nflags : fpu vmx\n")
        .unwrap();
    fs::write(root.join("meminfo"), "MemTotal:        2027168 kB\n").unwrap();
    fs::write(root.join("volume1/media/isos/debian.iso"), "iso").unwrap();
    let state = root.join("state");
    // Domains defined are files in state/; zvols too.
    let virsh = script(
        &root,
        "virsh",
        &format!(
            "state={state}\nshift 2\n\
             case \"$1\" in\n\
             domstate) test -f \"$state/$2\" ;;\n\
             define) name=$(sed -n 's/.*<name>\\(.*\\)<\\/name>.*/\\1/p' \"$3\"); touch \"$state/$name\" ;;\n\
             undefine) eval last=\\${{$#}}; rm \"$state/$last\" ;;\n\
             list) printf ' Id   Name                     State\\n----------------------------------------\\n'\n\
                   for f in \"$state\"/zendiloos-*; do [ -e \"$f\" ] && printf ' -    %s   shut off\\n' \"$(basename \"$f\")\"; done\n\
                   printf ' 3    win10                    running\\n' ;;\n\
             start) [ -f \"$state/running\" ] && {{ echo 'error: Requested operation is not valid: domain is already active' >&2; exit 1; }}; touch \"$state/running\" ;;\n\
             esac",
            state = state.display()
        ),
    );
    let zfs = script(
        &root,
        "zfs",
        &format!(
            "state={state}\n\
             case \"$*\" in\n\
             *'-s local -d 0'*) echo 1 ;;\n\
             'get -H -o value zendiloos:volume tank') echo 1 ;;\n\
             'list -H -o name -d 1 tank') echo tank; if [ -f \"$state/vms\" ]; then echo tank/zendiloos.vms; fi ;;\n\
             'list -H -o name tank/zendiloos.vms') test -f \"$state/vms\" ;;\n\
             'list -H -o name tank/zendiloos.vms/'*) test -f \"$state/zvol\" ;;\n\
             create*zendiloos.vms) touch \"$state/vms\" ;;\n\
             'create -s -V'*) touch \"$state/zvol\" ;;\n\
             'destroy tank/zendiloos.vms/'*) rm \"$state/zvol\" ;;\n\
             esac",
            state = state.display()
        ),
    );
    let systemd_run = script(
        &root,
        "systemd-run",
        "case \"$*\" in\n\
         *copy-vm-iso*) echo '{\"progress\":{\"step\":\"copying_iso\",\"percent\":40}}'; echo '{\"done\":{\"bytes\":3}}' ;;\n\
         esac",
    );
    let mut tools = Tools {
        virsh,
        zfs,
        zpool: script(&root, "zpool", ""),
        systemctl: script(&root, "systemctl", ""),
        systemd_run,
        libvirtd: root.join("libvirtd"),
        qemu: root.join("qemu-system-x86_64"),
        systemd_tmpfiles: PathBuf::from("/usr/bin/systemd-tmpfiles"),
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
            net: root.join("net"),
            domains: root.join("domains"),
            consoles: root.join("consoles"),
            tmpfiles: root.join("tmpfiles.d/zendiloos-vms.conf"),
            apparmor: root.join("apparmor/abstractions/libvirt-qemu.d/zendiloos-consoles"),
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

fn spec(network: VmNetwork, iso: bool, autostart: bool) -> VmSpec {
    VmSpec {
        id: id(),
        os: VmOs::Linux,
        cpus: VmCpus::new(1).unwrap(),
        memory: VmMemory::new(256).unwrap(),
        firmware: VmFirmware::Uefi,
        disk: VmDiskSize::new(1).unwrap(),
        network,
        iso,
        autostart,
        pool: pool(),
        volume: volume(),
    }
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
}

#[tokio::test]
async fn the_machine_says_what_it_offers() {
    let host = host(true);
    let Response::VmHost { kvm, cpu_virtualization, iommu, installed, cores, memory_bytes, bridges, storage } =
        host.run(Request::VmHost {}).await
    else {
        panic!("no facts")
    };
    assert!(kvm && installed && !iommu);
    assert_eq!(cpu_virtualization.as_deref(), Some("vmx"));
    assert_eq!((cores, memory_bytes), (2, 2_027_168 * 1024));
    assert_eq!(bridges, [InterfaceName::parse("br0").unwrap()]);
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
}

#[tokio::test]
async fn a_vm_gets_a_sparse_zvol_and_a_domain_of_its_own() {
    let host = host(true);
    let define = |spec: VmSpec| Request::DefineVm { spec };
    refused(host.run(define(spec(VmNetwork::User, false, false))).await, ErrorCode::Forbidden, "not set up");
    set_up(&host).await;

    refused(
        host.run(define(spec(VmNetwork::User, true, false))).await,
        ErrorCode::Forbidden,
        "ISO was not copied",
    );
    let card = VmNetwork::Bridge { bridge: InterfaceName::parse("enp1s0").unwrap() };
    refused(host.run(define(spec(card, false, false))).await, ErrorCode::Forbidden, "no bridge");
    assert!(!host.calls().iter().any(|call| call.starts_with("zfs create -s")), "nothing made for a refusal");

    let bridge = VmNetwork::Bridge { bridge: InterfaceName::parse("br0").unwrap() };
    assert_eq!(host.run(define(spec(bridge, false, true))).await, Response::Done);
    let calls = host.calls();
    assert!(
        calls.contains(&format!("zfs create -s -V 1G -o volmode=dev tank/zendiloos.vms/{ID}-disk0")),
        "{calls:?}"
    );
    let file = format!("/domains/zendiloos-{ID}.xml");
    assert!(calls.contains(&format!("virsh --connect qemu:///system define --validate {file}")), "{calls:?}");
    assert_eq!(calls.last().unwrap(), &format!("virsh --connect qemu:///system autostart zendiloos-{ID}"));
    let xml = fs::read_to_string(host.path(&file[1..])).unwrap();
    assert!(xml.contains(&format!("<source dev='/dev/zvol/tank/zendiloos.vms/{ID}-disk0'/>")), "{xml}");
    assert!(
        xml.contains("<source bridge='br0'/>") && xml.contains(&format!("consoles/{ID}.vnc'/>")),
        "{xml}"
    );

    refused(host.run(define(spec(VmNetwork::User, false, false))).await, ErrorCode::Conflict, "taken");

    assert_eq!(host.run(Request::SetVmAutostart { id: id(), autostart: false }).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        &format!("virsh --connect qemu:///system autostart --disable zendiloos-{ID}")
    );
}

#[tokio::test]
async fn an_iso_is_copied_by_a_unit_that_may_write_only_the_vms_folder() {
    let host = host(true);
    set_up(&host).await;
    let source = VmIsoSource {
        share: ShareLocation { volume: volume(), share: ShareName::parse("media").unwrap() },
        path: SharePath::parse("isos/debian.iso").unwrap(),
    };
    let copy = Request::CopyVmIso { id: id(), volume: volume(), source };
    assert_eq!(host.run(copy).await, Response::Done);
    let call = host.calls().into_iter().find(|call| call.contains("copy-vm-iso")).expect("a unit");
    for part in [
        &format!("--unit=zendiloos-vm-iso-{ID}.service") as &str,
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
async fn vms_are_started_and_stopped_only_by_their_domain() {
    let host = host(true);
    set_up(&host).await;
    let control = |action| Request::ControlVm { id: id(), action };
    refused(host.run(control(VmAction::Start)).await, ErrorCode::Forbidden, "no virtual machine");
    assert_eq!(
        host.run(Request::DefineVm { spec: spec(VmNetwork::User, false, false) }).await,
        Response::Done
    );

    for (action, args) in [
        (VmAction::Start, "start"),
        (VmAction::Shutdown, "shutdown --mode acpi"),
        (VmAction::Reboot, "reboot --mode acpi"),
        (VmAction::ForceOff, "destroy"),
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
    assert!(virsh_list.contains(&format!("zendiloos-{ID}   shut off")), "{virsh_list}");
}

#[tokio::test]
async fn deleting_a_vm_removes_its_domain_disk_iso_and_xml() {
    let host = host(true);
    set_up(&host).await;
    assert_eq!(
        host.run(Request::DefineVm { spec: spec(VmNetwork::User, false, false) }).await,
        Response::Done
    );
    fs::create_dir_all(host.path("volume1/@vms/isos")).unwrap();
    fs::write(host.path(&format!("volume1/@vms/isos/{ID}.iso")), "iso").unwrap();

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
    let rm = calls.iter().find(|call| call.contains("rm --force")).expect("the ISO removed");
    assert!(
        rm.contains("--property=ReadWritePaths=/volume1/@vms ")
            && rm.ends_with(&format!("-- /volume1/@vms/isos/{ID}.iso")),
        "{rm}"
    );
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
