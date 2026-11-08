//! Network configuration against a fake host: the networkd directory and
//! systemd-networkd's D-Bus API (busctl) replaced by scripts that record
//! their arguments.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, NetworkPaths, Operations, Paths};
use zendiloos_agent_proto::{
    ConfirmWithin, ErrorCode, InterfaceName, Ipv4Config, Ipv6Config, NameServers, NetworkConfig, Prefix,
    Request, Response, Role,
};

struct Host {
    _dir: TempDir,
    root: PathBuf,
    tools: Tools,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("network")).unwrap();
        let script = |name: &str, body: &str| -> PathBuf {
            let path = root.join(name);
            let log = root.join("calls.log");
            fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let any = script("tool", "");
        let tools = Tools {
            systemd_mount: "/bin/false".into(),
            umount: "/bin/false".into(),
            rmdir: "/bin/false".into(),
            systemctl: script("systemctl", "exit 0"),
            // systemd-networkd knows enp1s0 only, as link 2.
            busctl: script(
                "busctl",
                "case \"$5 $7\" in \"GetLinkByName enp1s0\") echo 'io 2 \"/org/x/_32\"';; GetLinkByName*) exit 1;; esac",
            ),
            lsblk: any.clone(),
            smartctl: any.clone(),
            zpool: any.clone(),
            zfs: any.clone(),
            sgdisk: any.clone(),
            wipefs: any.clone(),
            udevadm: any.clone(),
            testparm: any.clone(),
            exportfs: any.clone(),
            setfacl: any.clone(),
            useradd: any.clone(),
            userdel: any.clone(),
            groupadd: any.clone(),
            groupdel: any.clone(),
            gpasswd: any.clone(),
            smbpasswd: any.clone(),
            timedatectl: any.clone(),
            sshd: any.clone(),
            systemd_run: any.clone(),
            docker: any.clone(),
            setsid: "/usr/bin/setsid".into(),
            openssl: "/bin/false".into(),
            nginx: "/bin/false".into(),
            dockerd: any,
        };
        Self { _dir: dir, root, tools }
    }

    /// The agent as it starts: a new one stands for a restart.
    async fn agent(&self) -> Operations {
        let paths =
            Paths { network: NetworkPaths { directory: self.root.join("network") }, ..Paths::default() };
        let operations =
            Operations::with_paths(Role::System, &self.tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        operations.recover().await;
        operations
    }

    fn file(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join("network").join(name)).ok()
    }

    /// The methods called on systemd-networkd's manager.
    fn networkd(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        let manager = "busctl call org.freedesktop.network1 /org/freedesktop/network1 org.freedesktop.network1.Manager ";
        log.lines().filter_map(|l| l.strip_prefix(manager)).map(str::to_owned).collect()
    }
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn apply(address: &str, seconds: u16) -> Request {
    Request::ApplyNetwork {
        config: NetworkConfig {
            interface: InterfaceName::parse("enp1s0").unwrap(),
            ipv4: Ipv4Config::Static {
                address: address.parse().unwrap(),
                prefix: Prefix::new(24).unwrap(),
                gateway: Some("192.168.1.1".parse().unwrap()),
            },
            ipv6: Ipv6Config::Automatic,
            dns: NameServers::new(vec!["192.168.1.1".parse().unwrap()]).unwrap(),
        },
        confirm_within: ConfirmWithin::seconds(seconds).unwrap(),
    }
}

#[tokio::test]
async fn a_change_waits_for_confirmation_and_goes_back_if_none_comes() {
    let host = Host::new();
    let agent = host.agent().await;

    assert_eq!(agent.execute(&apply("192.168.1.10", 120)).await, Response::Done);
    let written = host.file("05-zendiloos-enp1s0.network").unwrap();
    assert!(written.contains("Address=192.168.1.10/24"), "{written}");
    assert!(
        host.file("05-zendiloos-enp1s0.network.previous")
            .unwrap()
            .starts_with("# zendiloos: there was no file")
    );
    assert_eq!(
        host.networkd(),
        ["GetLinkByName s enp1s0", "GetLinkByName s enp1s0", "Reload", "ReconfigureLink i 2"]
    );
    let Response::NetworkState { managed, pending } = agent.execute(&Request::NetworkState {}).await else {
        panic!("no state")
    };
    assert!(managed);
    assert_eq!(pending.unwrap().interface.as_str(), "enp1s0");
    let Response::Error { code, .. } = agent.execute(&apply("192.168.1.11", 120)).await else {
        panic!("a second change")
    };
    assert_eq!(code, ErrorCode::Conflict, "one change at a time");

    assert_eq!(agent.execute(&Request::ConfirmNetwork {}).await, Response::Done);
    assert!(host.file("05-zendiloos-enp1s0.network.previous").is_none(), "confirmed: nothing to go back to");
    assert!(matches!(agent.execute(&Request::ConfirmNetwork {}).await, Response::Error { .. }));

    // The next change is not confirmed before the agent restarts: the
    // confirmed file returns.
    assert_eq!(agent.execute(&apply("10.9.9.9", 120)).await, Response::Done);
    drop(agent);
    let _restarted = host.agent().await;
    let back = host.file("05-zendiloos-enp1s0.network").unwrap();
    assert!(back.contains("Address=192.168.1.10/24"), "{back}");
    assert!(!exists(&host.root.join("network/05-zendiloos-enp1s0.network.previous")));
}

#[tokio::test]
async fn the_first_change_unconfirmed_leaves_no_file() {
    let host = Host::new();
    let agent = host.agent().await;
    assert_eq!(agent.execute(&apply("192.168.1.10", 30)).await, Response::Done);
    drop(agent);

    let _restarted = host.agent().await;

    assert!(host.file("05-zendiloos-enp1s0.network").is_none());
    assert!(host.file("05-zendiloos-enp1s0.network.previous").is_none());
}

#[tokio::test]
async fn only_existing_interfaces_and_only_with_networkd() {
    let host = Host::new();
    let agent = host.agent().await;
    let mut other = apply("192.168.1.10", 30);
    if let Request::ApplyNetwork { config, .. } = &mut other {
        config.interface = InterfaceName::parse("eth9").unwrap();
    }
    assert!(matches!(agent.execute(&other).await, Response::Error { code: ErrorCode::Forbidden, .. }));

    let mut unmanaged = Host::new();
    let systemctl = unmanaged.root.join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\nexit 3\n").unwrap();
    unmanaged.tools.systemctl = systemctl;
    let agent = unmanaged.agent().await;
    let refused = agent.execute(&apply("192.168.1.10", 30)).await;
    assert!(matches!(refused, Response::Error { code: ErrorCode::Forbidden, .. }), "{refused:?}");
    assert!(unmanaged.file("05-zendiloos-enp1s0.network").is_none());
}
