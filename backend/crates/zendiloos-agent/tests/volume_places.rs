//! The system agent's two long places are for deploys, image pulls,
//! project control, removable drives, certificates and pools: counting the
//! volumes' data, backing them up and restoring them have places of their
//! own, so hours of volume work never keep those waiting.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{AccountFiles, Erasable, ErasePolicy, HostPaths, Operations, Paths, Server, serve};
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, ProjectConfig, ProjectName, Request, Response, Role, UnixTransport,
};

struct Agent {
    dir: TempDir,
    _stop: oneshot::Sender<()>,
}

impl Agent {
    /// Every program records its call; `docker` takes three seconds.
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let plain = |name: &str| script(name, "");
        let tools = Tools {
            openssl: "/bin/false".into(),
            nginx: "/bin/false".into(),
            rm: "/bin/false".into(),
            nft: "/bin/false".into(),
            virsh: "/bin/false".into(),
            libvirtd: "/bin/false".into(),
            qemu: "/bin/false".into(),
            systemd_tmpfiles: "/bin/false".into(),
            systemd_mount: "/bin/false".into(),
            umount: "/bin/false".into(),
            rmdir: "/bin/false".into(),
            systemctl: script("systemctl", "[ \"$1\" = is-active ] && exit 1\nexit 0"),
            lsblk: script("lsblk", "echo '{\"blockdevices\": []}'"),
            smartctl: plain("smartctl"),
            zpool: plain("zpool"),
            zfs: plain("zfs"),
            sgdisk: plain("sgdisk"),
            busctl: plain("busctl"),
            timedatectl: plain("timedatectl"),
            sshd: plain("sshd"),
            systemd_run: plain("systemd-run"),
            docker: script("docker", "sleep 3"),
            setsid: "/usr/bin/setsid".into(),
            dockerd: plain("dockerd"),
            wipefs: plain("wipefs"),
            udevadm: plain("udevadm"),
            testparm: plain("testparm"),
            exportfs: plain("exportfs"),
            smbcontrol: plain("smbcontrol"),
            useradd: plain("useradd"),
            userdel: plain("userdel"),
            groupadd: plain("groupadd"),
            groupdel: plain("groupdel"),
            gpasswd: plain("gpasswd"),
            smbpasswd: plain("smbpasswd"),
        };
        let files = AccountFiles {
            passwd: dir.path().join("passwd"),
            group: dir.path().join("group"),
            high_water: dir.path().join("id-high-water"),
        };
        fs::write(&files.passwd, "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        fs::write(&files.group, "root:x:0:\n").unwrap();
        let our_uid = fs::metadata(dir.path()).unwrap().uid();
        let host = HostPaths { projects: dir.path().join("projects"), ..HostPaths::default() };
        let paths = Paths { accounts: files, host, ..Paths::default() };
        let operations =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
        let listener = UnixListener::bind(dir.path().join("agent.sock")).unwrap();
        let (stop, stopped) = oneshot::channel();
        tokio::spawn(serve(listener, Server::new(operations, our_uid), async {
            let _ = stopped.await;
        }));
        Self { dir, _stop: stop }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("agent.sock")
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }
}

#[tokio::test]
async fn counting_the_volumes_leaves_the_long_places_to_a_deploy() {
    let agent = Agent::start().await;
    // Two counts of the volumes' data, each three seconds of `docker`.
    let busy: Vec<_> = (0..2)
        .map(|_| {
            let client = UnixTransport::new(agent.socket(), Duration::from_secs(30));
            tokio::spawn(async move { client.call(Request::DockerVolumeSizes {}, None).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // A deploy (an image update on schedule, a redeploy) would wait behind
    // them, and be given up once half its client's patience is spent.
    let config = ProjectConfig::parse(r#"{"name":"web","services":{"web":{"image":"nginx"}}}"#).unwrap();
    let deploy = Request::DeployProject { project: ProjectName::parse("web").unwrap(), config, pull: false };
    // `docker` takes three seconds here: the client may well give up on
    // the deploy's answer, but not before it ran.
    let client = UnixTransport::new(agent.socket(), Duration::from_millis(1500));
    let answer = client.call(deploy, None).await;
    for request in busy {
        let _ = request.await;
    }
    let compose_ran = agent.calls().contains("docker compose");
    assert!(
        !matches!(&answer, Ok(Response::Error { code: ErrorCode::Failed, message }) if message.contains("not carried out")),
        "the deploy waited behind volume work and was dropped ({answer:?})"
    );
    assert!(compose_ran, "the deploy ran at once: {answer:?}");
}
