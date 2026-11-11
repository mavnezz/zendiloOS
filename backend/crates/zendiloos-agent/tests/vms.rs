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

    /// The zvols there are, by name: `tank/zendiloos.vms/<id>-disk0`.
    fn zvols(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.path("state/zvols"))
            .map(|entries| {
                entries.flatten().map(|e| e.file_name().to_string_lossy().replace('+', "/")).collect()
            })
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

/// The pools `tank` (volume 1) and `cold` (volume 2), zendiloOS's; libvirt
/// and QEMU installed unless `installed` is false. systemd-networkd knows
/// the host's links: a bridge `br0`, a card `enp1s0`, a bridge `vms0` it
/// does not describe, a bond `bond0`, a VLAN `enp1s0.10`, a `veth` and the
/// internal network `vmnet0`, which zendiloOS's files make a bridge
/// without members or an address.
fn host(installed: bool) -> Host {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for sub in ["units", "iommu", "state/zvols", "volume1/media/isos", "network"] {
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
    // Datasets are files state/vms.<pool>; zvols files in state/zvols
    // named by the zvol with + for /, holding their size.
    let zfs = script(
        &root,
        "zfs",
        &format!(
            "state={state}\neval last=\\${{$#}}\nzv=\"$state/zvols/$(echo \"$last\" | tr / +)\"\n\
             case \"$*\" in\n\
             *'-s local -d 0'*) echo 1 ;;\n\
             'get -H -o value zendiloos:volume tank') echo 1 ;;\n\
             'get -H -o value zendiloos:volume cold') echo 2 ;;\n\
             'list -H -o name -d 1 '*) echo \"$last\"; if [ -f \"$state/vms.$last\" ]; then echo \"$last/zendiloos.vms\"; fi ;;\n\
             'list -H -o name -t volume -r '*) for f in \"$state/zvols/${{last%%/*}}+\"*; do [ -e \"$f\" ] && basename \"$f\" | tr + /; done; true ;;\n\
             'list -H -o name '*/zendiloos.vms) test -f \"$state/vms.${{last%%/*}}\" ;;\n\
             'list -H -o name '*/zendiloos.vms/*) test -f \"$zv\" ;;\n\
             'list -H -p -r -t filesystem,volume '*) for d in \"$@\"; do case \"$d\" in */zendiloos.vms) \
                 printf '%s\\tfilesystem\\t4096\\t1024\\t1024\\t1000000\\t-\\n' \"$d\"; \
                 for f in \"$state/zvols/${{d%%/*}}+\"*; do [ -e \"$f\" ] && printf '%s\\tvolume\\t2048\\t2048\\t2048\\t1000000\\t%s\\n' \"$(basename \"$f\" | tr + /)\" \"$(cat \"$f\")\"; done ;; esac; done ;;\n\
             'get -H -p -o value volsize '*) cat \"$zv\" ;;\n\
             'create -s -V '*) echo $(( ${{4%G}} * 1073741824 )) > \"$zv\" ;;\n\
             create*zendiloos.vms) touch \"$state/vms.${{last%%/*}}\" ;;\n\
             'set volsize='*) v=${{2#volsize=}}; echo $(( ${{v%G}} * 1073741824 )) > \"$zv\" ;;\n\
             'destroy '*/zendiloos.vms/*) rm \"$zv\" ;;\n\
             'destroy '*/zendiloos.vms) rm \"$state/vms.${{last%%/*}}\" ;;\n\
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
    let link = |index: u8, kind: Option<&str>, kind_of_type: &str| {
        let kind = kind.map(|kind| format!(",\\\"Kind\\\":\\\"{kind}\\\"")).unwrap_or_default();
        format!(
            "*'DescribeLink i {index}') echo '{{\"type\":\"s\",\"data\":[\"{{\\\"Index\\\":{index}{kind},\\\"Type\\\":\\\"{kind_of_type}\\\"}}\"]}}' ;;\n"
        )
    };
    let busctl = script(
        &root,
        "busctl",
        &format!(
            "case \"$*\" in\n\
             *'GetLinkByName s br0') echo 'io 5 \"/org/freedesktop/network1/link/_35\"' ;;\n\
             *'GetLinkByName s enp1s0') echo 'io 2 \"/org/freedesktop/network1/link/_32\"' ;;\n\
             *'GetLinkByName s vms0') echo 'io 6 \"/org/freedesktop/network1/link/_36\"' ;;\n\
             *'GetLinkByName s bond0') echo 'io 7 \"/org/freedesktop/network1/link/_37\"' ;;\n\
             *'GetLinkByName s enp1s0.10') echo 'io 8 \"/org/freedesktop/network1/link/_38\"' ;;\n\
             *'GetLinkByName s veth1a2b') echo 'io 9 \"/org/freedesktop/network1/link/_39\"' ;;\n\
             *'GetLinkByName s vmnet0') echo 'io 10 \"/org/freedesktop/network1/link/_310\"' ;;\n\
             {}{}{}{}{}{}\
             *) exit 1 ;;\n\
             esac",
            link(5, Some("bridge"), "bridge"),
            link(2, None, "ether"),
            link(7, Some("bond"), "ether"),
            link(8, Some("vlan"), "ether"),
            link(9, Some("veth"), "ether"),
            link(10, Some("bridge"), "bridge"),
        ),
    );
    // vmnet0: a bridge of zendiloOS's without members or an address.
    fs::write(root.join("network/05-zendiloos-vmnet0.netdev"), "[NetDev]\nName=vmnet0\nKind=bridge\n")
        .unwrap();
    fs::write(
        root.join("network/05-zendiloos-vmnet0.network"),
        "[Match]\nName=vmnet0\n\n[Network]\nDHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no\n",
    )
    .unwrap();
    fs::write(root.join("network/05-zendiloos-br0.netdev"), "[NetDev]\nName=br0\nKind=bridge\n").unwrap();
    fs::write(root.join("network/05-zendiloos-enp1s0.network"), "[Network]\nBridge=br0\n").unwrap();
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
            network: root.join("network"),
            internal: root.join("internal-networks.nft"),
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

fn tank() -> VmStorage {
    VmStorage { pool: pool(), volume: volume() }
}

/// The second location.
fn cold() -> VmStorage {
    VmStorage { pool: PoolName::parse("cold").unwrap(), volume: VolumeNumber::new(2).unwrap() }
}

fn id() -> VmId {
    VmId::parse(ID).unwrap()
}

fn image() -> VmImageId {
    VmImageId::parse(IMAGE).unwrap()
}

fn disk(slot: u8, gib: u32) -> VmDisk {
    disk_on(slot, gib, tank())
}

fn disk_on(slot: u8, gib: u32, location: VmStorage) -> VmDisk {
    VmDisk {
        slot: VmDiskSlot::new(slot).unwrap(),
        size: VmDiskSize::new(gib).unwrap(),
        bus: VmDiskBus::Virtio,
        location,
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

/// The second location, on `cold`.
async fn add_cold(host: &Host) {
    let location = cold();
    assert_eq!(
        host.run(Request::EnableVms { pool: location.pool, volume: location.volume }).await,
        Response::Done
    );
    fs::create_dir_all(host.path("volume2/@vms")).unwrap();
}

#[tokio::test]
async fn the_machine_says_what_it_offers() {
    let host = host(true);
    let Response::VmHost { kvm, cpu_virtualization, iommu, installed, cores, memory_bytes, locations } =
        host.run(Request::VmHost {}).await
    else {
        panic!("no facts")
    };
    assert!(kvm && installed && !iommu);
    assert_eq!(cpu_virtualization.as_deref(), Some("vmx"));
    assert_eq!((cores, memory_bytes), (2, 2_027_168 * 1024));
    assert!(locations.is_empty());

    add_cold(&host).await;
    set_up(&host).await;
    let Response::VmHost { locations, .. } = host.run(Request::VmHost {}).await else { panic!("no facts") };
    assert_eq!(locations, [tank(), cold()], "by volume");
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

    // A second location, on another pool: libvirt waits for both, and
    // neither pool is destroyed.
    add_cold(&host).await;
    let drop_in = fs::read_to_string(host.path("units/libvirtd.service.d/zendiloos.conf")).unwrap();
    assert!(
        drop_in.contains("RequiresMountsFor=/volume1/@vms\nRequiresMountsFor=/volume2/@vms\n"),
        "{drop_in}"
    );
    let unit = fs::read_to_string(host.path("units/volume2-\\x40vms.mount")).unwrap();
    assert!(unit.contains("What=cold/zendiloos.vms\nWhere=/volume2/@vms\n"), "{unit}");
    for location in [tank(), cold()] {
        let destroy = host.run(Request::DestroyPool { name: location.pool, volume: location.volume }).await;
        refused(destroy, ErrorCode::Forbidden, "virtual machines' storage lies on this pool");
    }
    // Set up again is no failure.
    assert_eq!(host.run(Request::EnableVms { pool: pool(), volume: volume() }).await, Response::Done);
    // A pool not zendiloOS's holds none.
    let foreign =
        Request::EnableVms { pool: PoolName::parse("other").unwrap(), volume: VolumeNumber::new(3).unwrap() };
    assert!(matches!(host.run(foreign).await, Response::Error { .. }));
    // Two units naming one pool are a hand's work: neither counts.
    fs::write(host.path("units/volume4-\\x40vms.mount"), "[Mount]\nWhat=cold/zendiloos.vms\n").unwrap();
    let Response::VmHost { locations, .. } = host.run(Request::VmHost {}).await else { panic!("no facts") };
    assert_eq!(locations, [tank()]);
}

#[tokio::test]
async fn a_location_goes_only_once_empty_and_libvirt_stops_waiting_for_it_first() {
    let host = host(true);
    set_up(&host).await;
    add_cold(&host).await;
    let remove = |location: VmStorage| Request::DisableVms { pool: location.pool, volume: location.volume };

    // A VM's disk on it keeps it.
    let on_cold = VmDevices { disks: vec![disk_on(0, 1, cold())], ..devices() };
    assert_eq!(host.run(Request::DefineVm { spec: spec_with(on_cold, false) }).await, Response::Done);
    let from = host.calls().len();
    refused(host.run(remove(cold())).await, ErrorCode::Forbidden, &format!("cold/zendiloos.vms/{ID}-disk0"));
    assert!(!host.calls_since(from).iter().any(|call| call.contains("stop ") || call.contains("destroy")));
    assert_eq!(host.run(Request::DeleteVm { id: id() }).await, Response::Done);

    // So does an image in its library.
    fs::create_dir_all(host.path("volume2/@vms/isos")).unwrap();
    fs::write(host.path(&format!("volume2/@vms/isos/{IMAGE}.iso")), "iso").unwrap();
    refused(host.run(remove(cold())).await, ErrorCode::Forbidden, "holds 1 images");
    fs::remove_file(host.path(&format!("volume2/@vms/isos/{IMAGE}.iso"))).unwrap();
    // A file the library left half written is none.
    fs::write(host.path(&format!("volume2/@vms/isos/.{IMAGE}.iso.part")), "is").unwrap();

    let from = host.calls().len();
    assert_eq!(host.run(remove(cold())).await, Response::Done);
    let calls = host.calls_since(from);
    let at = |wanted: &str| {
        calls.iter().position(|call| call == wanted).unwrap_or_else(|| panic!("{wanted}: {calls:?}"))
    };
    let reloaded = at("systemctl daemon-reload");
    let stopped = at("systemctl stop volume2-\\x40vms.mount");
    let destroyed = at("zfs destroy cold/zendiloos.vms");
    let disabled = at("systemctl disable --now volume2-\\x40vms.mount");
    assert!(reloaded < stopped && stopped < destroyed && destroyed < disabled, "{calls:?}");
    assert!(!host.path("units/volume2-\\x40vms.mount").exists());
    let drop_in = fs::read_to_string(host.path("units/libvirtd.service.d/zendiloos.conf")).unwrap();
    assert!(
        drop_in.contains("RequiresMountsFor=/volume1/@vms\n") && !drop_in.contains("volume2"),
        "{drop_in}"
    );
    refused(host.run(remove(cold())).await, ErrorCode::Forbidden, "no location");

    // The last one too, and libvirt waits for none.
    assert_eq!(host.run(remove(tank())).await, Response::Done);
    assert!(!host.path("units/libvirtd.service.d/zendiloos.conf").exists());
    let Response::VmHost { locations, .. } = host.run(Request::VmHost {}).await else { panic!("no facts") };
    assert!(locations.is_empty());
}

#[tokio::test]
async fn a_busy_location_stays_with_libvirt_waiting_for_it() {
    let host = host(true);
    set_up(&host).await;
    add_cold(&host).await;
    // The mount is in use: systemctl fails to stop it.
    let systemctl = host.path("systemctl");
    let text = fs::read_to_string(&systemctl).unwrap();
    fs::write(&systemctl, format!("{text}\ncase \"$*\" in 'stop volume2-'*) exit 1 ;; esac\n")).unwrap();
    let location = cold();
    let refusal = host.run(Request::DisableVms { pool: location.pool, volume: location.volume }).await;
    assert!(matches!(refusal, Response::Error { .. }), "{refusal:?}");
    assert!(host.path("units/volume2-\\x40vms.mount").exists());
    assert!(host.zvols().is_empty() && host.path("state/vms.cold").exists(), "nothing destroyed");
    let drop_in = fs::read_to_string(host.path("units/libvirtd.service.d/zendiloos.conf")).unwrap();
    assert!(drop_in.contains("RequiresMountsFor=/volume2/@vms"), "waits for it again: {drop_in}");
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
    assert_eq!(
        host.zvols(),
        [format!("tank/zendiloos.vms/{ID}-disk0"), format!("tank/zendiloos.vms/{ID}-disk1")]
    );
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
    assert_eq!(
        host.zvols(),
        [format!("tank/zendiloos.vms/{ID}-disk0"), format!("tank/zendiloos.vms/{ID}-disk2")]
    );
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
    let changing = |call: &String| {
        ["zfs create", "zfs set", "zfs destroy"].iter().any(|start| call.starts_with(start))
            || call.contains(" define ")
    };
    assert!(!host.calls_since(from).iter().any(changing));

    let other = VmSpec::new(
        VmId::parse("00000000000f").unwrap(),
        VmOs::Linux,
        VmCpus::new(1).unwrap(),
        VmMemory::new(256).unwrap(),
        VmFirmware::Uefi,
        devices(),
        false,
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
    refused(host.run(elsewhere).await, ErrorCode::Forbidden, "on volume 2");
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

    assert_eq!(host.run(Request::DeleteVm { id: id() }).await, Response::Done);
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
    assert_eq!(host.run(Request::DeleteVm { id: id() }).await, Response::Done);
    // Without storage there is nothing to delete from.
    let bare = self::host(true);
    refused(bare.run(Request::DeleteVm { id: id() }).await, ErrorCode::Forbidden, "not set up");
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

/// A VM's disks on two locations: each zvol on its pool, each found by the
/// domain there; deleted from both, and what they hold told per location.
#[tokio::test]
async fn disks_lie_on_the_locations_they_were_made_on() {
    let host = host(true);
    set_up(&host).await;
    let two = VmDevices { disks: vec![disk(0, 1), disk_on(1, 2, cold())], ..devices() };
    // Not before the second location is there.
    refused(
        host.run(Request::DefineVm { spec: spec_with(two.clone(), false) }).await,
        ErrorCode::Forbidden,
        "no location of the virtual machines' storage is on volume 2",
    );
    add_cold(&host).await;
    let elsewhere = VmDevices {
        disks: vec![disk_on(
            1,
            2,
            VmStorage { pool: PoolName::parse("tank").unwrap(), volume: VolumeNumber::new(2).unwrap() },
        )],
        boot: vec![VmBoot::Disk(VmDiskSlot::new(1).unwrap())],
        ..devices()
    };
    refused(
        host.run(Request::DefineVm { spec: spec_with(elsewhere, false) }).await,
        ErrorCode::Forbidden,
        "is not on pool tank",
    );
    assert_eq!(host.run(Request::DefineVm { spec: spec_with(two.clone(), false) }).await, Response::Done);
    assert_eq!(
        host.zvols(),
        [format!("cold/zendiloos.vms/{ID}-disk1"), format!("tank/zendiloos.vms/{ID}-disk0")]
    );
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(xml.contains(&format!("<source dev='/dev/zvol/tank/zendiloos.vms/{ID}-disk0'/>")), "{xml}");
    assert!(xml.contains(&format!("<source dev='/dev/zvol/cold/zendiloos.vms/{ID}-disk1'/>")), "{xml}");

    // What each location and its zvols hold, in one look.
    let Response::VmStorageUsage { zfs_list } = host.run(Request::VmStorageUsage {}).await else {
        panic!("no usage")
    };
    assert!(zfs_list.contains("tank/zendiloos.vms\tfilesystem"), "{zfs_list}");
    assert!(zfs_list.contains(&format!("cold/zendiloos.vms/{ID}-disk1\tvolume\t2048")), "{zfs_list}");
    let looked = host.calls().into_iter().rfind(|call| call.starts_with("zfs list -H -p -r")).unwrap();
    assert_eq!(
        looked,
        "zfs list -H -p -r -t filesystem,volume -o name,type,used,usedbydataset,referenced,available,volsize \
         tank/zendiloos.vms cold/zendiloos.vms"
    );

    // A disk stays where it was made: moved, it is refused; grown, it grows there.
    let slot = |n| VmDiskSlot::new(n).unwrap();
    let moved = VmDevices { disks: vec![disk(0, 1), disk(1, 2)], ..devices() };
    refused(
        host.run(Request::UpdateVm { spec: spec_with(moved, false), remove: vec![] }).await,
        ErrorCode::Forbidden,
        &format!("disk 1 lies on cold/zendiloos.vms/{ID}-disk1"),
    );
    let grown = VmDevices { disks: vec![disk(0, 1), disk_on(1, 3, cold())], ..devices() };
    assert_eq!(
        host.run(Request::UpdateVm { spec: spec_with(grown, false), remove: vec![] }).await,
        Response::Done
    );
    assert!(host.calls().contains(&format!("zfs set volsize=3G cold/zendiloos.vms/{ID}-disk1")));
    // Removed: destroyed on its own location.
    let one = VmDevices { disks: vec![disk(0, 1)], ..devices() };
    assert_eq!(
        host.run(Request::UpdateVm { spec: spec_with(one, false), remove: vec![slot(1)] }).await,
        Response::Done
    );
    assert!(host.calls().contains(&format!("zfs destroy cold/zendiloos.vms/{ID}-disk1")));
    assert_eq!(host.zvols(), [format!("tank/zendiloos.vms/{ID}-disk0")]);

    assert_eq!(
        host.run(Request::UpdateVm { spec: spec_with(two, false), remove: vec![] }).await,
        Response::Done
    );
    assert_eq!(host.run(Request::DeleteVm { id: id() }).await, Response::Done);
    assert!(host.zvols().is_empty(), "both locations: {:?}", host.zvols());
}

/// Directly on a card (macvtap): a wired card or a bond, as systemd-networkd
/// describes it, never a bridge, VLAN, veth or a link it does not know.
#[tokio::test]
async fn a_card_goes_directly_only_on_a_wired_card_or_bond() {
    let host = host(true);
    set_up(&host).await;
    let direct = |card: &str| VmDevices {
        nics: vec![nic(0, VmNetwork::Direct { card: InterfaceName::parse(card).unwrap() })],
        ..devices()
    };
    for other in ["br0", "enp1s0.10", "veth1a2b", "eth9"] {
        refused(
            host.run(Request::DefineVm { spec: spec_with(direct(other), false) }).await,
            ErrorCode::Forbidden,
            "no wired card or bond",
        );
    }
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_with(direct("enp1s0"), false) }).await,
        Response::Done
    );
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(
        xml.contains(
            "<interface type='direct'>\n      <mac address='52:54:00:00:00:00'/>\n      \
             <source dev='enp1s0' mode='bridge'/>\n      <model type='virtio'/>\n    </interface>"
        ),
        "{xml}"
    );
    assert!(!xml.contains("type='network'"), "{xml}");
    let bonded = spec_with(direct("bond0"), false);
    assert_eq!(host.run(Request::UpdateVm { spec: bonded, remove: vec![] }).await, Response::Done);
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(xml.contains("<source dev='bond0' mode='bridge'/>"), "{xml}");
}

/// An internal network: a bridge of zendiloOS's without members or an
/// address, taken only while the rules that keep the NAS off it are in
/// force; on the domain, a bridge as any other.
#[tokio::test]
async fn a_card_goes_on_an_internal_network_only_while_the_nas_is_kept_off_it() {
    let host = host(true);
    set_up(&host).await;
    let internal = |bridge: &str| VmDevices {
        nics: vec![nic(0, VmNetwork::Internal { bridge: InterfaceName::parse(bridge).unwrap() })],
        ..devices()
    };
    // Not written yet.
    refused(
        host.run(Request::DefineVm { spec: spec_with(internal("vmnet0"), false) }).await,
        ErrorCode::Forbidden,
        "no internal network the NAS keeps itself off",
    );
    fs::write(
        host.path("internal-networks.nft"),
        "# Written by zendiloOS: what the NAS keeps off its internal networks for VMs.\n\
         # Changes made here are replaced.\n\
         table bridge zendiloos_internal\ndelete table bridge zendiloos_internal\n\
         table bridge zendiloos_internal {\n\
         \tchain input {\n\t\ttype filter hook input priority filter; policy accept;\n\
         \t\t# Nothing on an internal network reaches the NAS.\n\t\tmeta ibrname { \"vmnet0\" } drop\n\t}\n\
         \tchain output {\n\t\ttype filter hook output priority filter; policy accept;\n\
         \t\t# Nor does the NAS send anything onto one.\n\t\tmeta obrname { \"vmnet0\" } drop\n\t}\n}\n",
    )
    .unwrap();
    // br0 has a card: the LAN's, no internal network.
    refused(
        host.run(Request::DefineVm { spec: spec_with(internal("br0"), false) }).await,
        ErrorCode::Forbidden,
        "br0 is no internal network",
    );
    refused(
        host.run(Request::DefineVm { spec: spec_with(internal("enp1s0"), false) }).await,
        ErrorCode::Forbidden,
        "no bridge",
    );
    assert_eq!(
        host.run(Request::DefineVm { spec: spec_with(internal("vmnet0"), false) }).await,
        Response::Done
    );
    let xml = fs::read_to_string(host.path(&format!("domains/zendiloos-{ID}.xml"))).unwrap();
    assert!(xml.contains("<interface type='bridge'>\n      <mac address='52:54:00:00:00:00'/>\n      <source bridge='vmnet0'/>"), "{xml}");
    assert!(
        host.calls().contains(&"systemctl is-active --quiet zendiloos-internal-networks.service".to_owned())
    );

    // A card put in it since: the rules no longer those for the files, nor is it internal.
    fs::write(host.path("network/05-zendiloos-enp2s0.network"), "[Network]\nBridge=vmnet0\n").unwrap();
    refused(
        host.run(Request::UpdateVm { spec: spec_with(internal("vmnet0"), false), remove: vec![] }).await,
        ErrorCode::Forbidden,
        "no internal network",
    );
}
