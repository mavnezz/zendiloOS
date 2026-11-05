//! The agent behind a real Unix socket, talked to with the client
//! zendiloosd uses. Programs are replaced by scripts that record calls.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{AccountFiles, Erasable, ErasePolicy, HostPaths, Operations, Paths, Server, serve};
use zendiloos_agent_proto::{
    AgentTransport, DiskName, ErrorCode, GroupName, LSBLK_COLUMNS, PROTOCOL_VERSION, Password, Request,
    RequestId, Response, UnixTransport, UserName, ZFS_DATASET_COLUMNS,
};

/// A Debian system with the owner's user, one managed user and one managed
/// group.
const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash
debian:x:1000:1000:Debian:/home/debian:/bin/bash
zendiloos:x:996:993:zendiloOS:/var/lib/zendiloos:/usr/sbin/nologin
alice:x:2001:990::/nonexistent:/usr/sbin/nologin
";
const GROUP: &str = "root:x:0:
sudo:x:27:debian
debian:x:1000:
zendiloos-users:x:990:
family:x:2001:alice
";

struct Agent {
    dir: TempDir,
    stop: Option<oneshot::Sender<()>>,
}

impl Agent {
    /// Starts an agent serving `client_uid`. Every program is a script that
    /// appends its name and arguments to `calls.log`; `systemctl` exits with
    /// `exit_code`, `lsblk` prints an empty device list and `smartctl`
    /// prints a JSON object and exits with 4 (a SMART command failed).
    async fn start(client_uid: Option<u32>, exit_code: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            fs::write(&path, format!("#!/bin/sh\necho {name} \"$@\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let tools = Tools {
            systemctl: script("systemctl", &format!("exit {exit_code}")),
            lsblk: script("lsblk", "echo '{\"blockdevices\": []}'"),
            smartctl: script("smartctl", "echo '{\"smart_status\": {\"passed\": true}}'; exit 4"),
            zpool: script("zpool", "echo '{\"pools\": {}}'"),
            zfs: script("zfs", "echo '{\"datasets\": {}}'"),
            sgdisk: script("sgdisk", ""),
            wipefs: script("wipefs", ""),
            udevadm: script("udevadm", ""),
            useradd: script("useradd", ""),
            userdel: script("userdel", ""),
            groupadd: script("groupadd", ""),
            groupdel: script("groupdel", ""),
            gpasswd: script("gpasswd", ""),
            // Only shell builtins: the environment is empty, so there is no PATH.
            smbpasswd: script(
                "smbpasswd",
                &format!(
                    "while IFS= read -r line; do printf '%s\\n' \"$line\" >> {}; done",
                    dir.path().join("stdin.log").display()
                ),
            ),
        };
        let files = AccountFiles { passwd: dir.path().join("passwd"), group: dir.path().join("group") };
        fs::write(&files.passwd, PASSWD).unwrap();
        fs::write(&files.group, GROUP).unwrap();

        // Files we create belong to us: the test runs as the client.
        let our_uid = fs::metadata(dir.path()).unwrap().uid();
        let paths = Paths { accounts: files, host: HostPaths::default() };
        let operations = Operations::with_paths(&tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
        let listener = UnixListener::bind(dir.path().join("agent.sock")).unwrap();
        let (stop, stopped) = oneshot::channel();
        let server = Server::new(operations, client_uid.unwrap_or(our_uid));
        tokio::spawn(serve(listener, server, async {
            let _ = stopped.await;
        }));
        Self { dir, stop: Some(stop) }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("agent.sock")
    }

    fn client(&self) -> UnixTransport {
        UnixTransport::new(self.socket(), Duration::from_secs(5))
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    fn stdin(&self) -> String {
        fs::read_to_string(self.dir.path().join("stdin.log")).unwrap_or_default()
    }

    /// Sends raw bytes and returns the raw answer.
    async fn raw(&self, bytes: &[u8]) -> String {
        let mut stream = UnixStream::connect(self.socket()).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn assert_error(response: Response, expected: ErrorCode) {
    assert!(matches!(response, Response::Error { code, .. } if code == expected), "{response:?}");
}

fn id(value: &str) -> Option<RequestId> {
    Some(RequestId::parse(value).unwrap())
}

#[tokio::test]
async fn answers_the_handshake() {
    let agent = Agent::start(None, 0).await;
    let response = agent.client().call(Request::Hello {}, id("req-1")).await.unwrap();
    assert_eq!(
        response,
        Response::Hello {
            protocol_version: PROTOCOL_VERSION,
            agent_version: env!("CARGO_PKG_VERSION").into()
        }
    );
    assert_eq!(agent.calls(), "", "the handshake runs nothing");
}

#[tokio::test]
async fn reboots_and_powers_off_through_systemctl() {
    let agent = Agent::start(None, 0).await;
    assert_eq!(agent.client().call(Request::Reboot {}, id("req-1")).await.unwrap(), Response::Done);
    assert_eq!(agent.client().call(Request::PowerOff {}, None).await.unwrap(), Response::Done);
    assert_eq!(agent.calls(), "systemctl reboot\nsystemctl poweroff\n");
}

#[tokio::test]
async fn lists_block_devices_with_fixed_columns() {
    let agent = Agent::start(None, 0).await;
    let response = agent.client().call(Request::ListBlockDevices {}, None).await.unwrap();
    assert_eq!(response, Response::BlockDevices { lsblk_json: "{\"blockdevices\": []}\n".into() });
    assert_eq!(agent.calls(), format!("lsblk --json --bytes --tree --output {LSBLK_COLUMNS}\n"));
}

#[tokio::test]
async fn reads_the_raid_status_without_running_anything() {
    let agent = Agent::start(None, 0).await;
    let response = agent.client().call(Request::ArrayStatus {}, None).await.unwrap();
    assert!(matches!(response, Response::ArrayStatus { .. }), "{response:?}");
    assert_eq!(agent.calls(), "");
}

#[tokio::test]
async fn reads_zfs_status_as_json() {
    let agent = Agent::start(None, 0).await;
    let response = agent.client().call(Request::ZfsStatus {}, None).await.unwrap();
    assert_eq!(
        response,
        Response::Zfs { zpool_status: "{\"pools\": {}}\n".into(), zfs_list: "{\"datasets\": {}}\n".into() }
    );
    assert_eq!(
        agent.calls(),
        format!("zpool status -j --json-int\nzfs list -j --json-int -d 0 -o {ZFS_DATASET_COLUMNS}\n")
    );
}

#[tokio::test]
async fn passes_on_smartctl_output_and_exit_status() {
    let agent = Agent::start(None, 0).await;
    let device = DiskName::parse("nvme0n1").unwrap();
    let response = agent.client().call(Request::SmartData { device }, None).await.unwrap();
    assert_eq!(
        response,
        Response::Smart { exit_status: 4, smartctl_json: "{\"smart_status\": {\"passed\": true}}\n".into() }
    );
    assert_eq!(agent.calls(), "smartctl --json=c --all --nocheck=standby /dev/nvme0n1\n");
}

#[tokio::test]
async fn reports_a_failing_program() {
    let agent = Agent::start(None, 1).await;
    assert_error(agent.client().call(Request::Reboot {}, None).await.unwrap(), ErrorCode::Failed);
}

#[tokio::test]
async fn serves_only_its_client_account() {
    let our_uid = fs::metadata(".").unwrap().uid();
    let agent = Agent::start(Some(our_uid.wrapping_add(1)), 0).await;
    assert_error(agent.client().call(Request::Reboot {}, None).await.unwrap(), ErrorCode::Forbidden);
    assert_eq!(agent.calls(), "", "nothing ran");
}

#[tokio::test]
async fn refuses_requests_outside_the_protocol() {
    let agent = Agent::start(None, 0).await;
    for request in [
        "not json\n".to_owned(),
        "{\"version\":1,\"request\":{\"op\":\"run\",\"command\":\"id\"}}\n".to_owned(),
        "{\"version\":1,\"request\":{\"op\":\"reboot\",\"force\":true}}\n".to_owned(),
        format!("{{\"version\":1,\"request\":{{\"op\":\"hello\",\"pad\":\"{}\"}}}}\n", "x".repeat(70_000)),
    ] {
        let answer = agent.raw(request.as_bytes()).await;
        assert!(answer.contains("\"code\":\"invalid_request\""), "{answer}");
    }
    let other_version = agent.raw(b"{\"version\":99,\"request\":{\"op\":\"reboot\"}}\n").await;
    assert!(other_version.contains("\"code\":\"version_mismatch\""), "{other_version}");
    assert_eq!(agent.calls(), "", "nothing ran");
}

#[tokio::test]
async fn the_client_reports_a_missing_agent() {
    let error = UnixTransport::new(Path::new("/nonexistent/agent.sock"), Duration::from_secs(1))
        .call(Request::Hello {}, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cannot connect"), "{error}");
}

fn user(name: &str) -> UserName {
    UserName::parse(name).unwrap()
}

fn group(name: &str) -> GroupName {
    GroupName::parse(name).unwrap()
}

#[tokio::test]
async fn creates_users_in_the_managed_range_only() {
    let agent = Agent::start(None, 0).await;
    let created = agent.client().call(Request::CreateUser { name: user("carol") }, None).await.unwrap();
    assert_eq!(created, Response::Done);
    assert_eq!(
        agent.calls(),
        "useradd --no-log-init --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin \
         --gid zendiloos-users --key UID_MIN=2000 --key UID_MAX=59999 -- carol\n"
    );

    for taken in ["debian", "alice", "root"] {
        let response = agent.client().call(Request::CreateUser { name: user(taken) }, None).await.unwrap();
        assert_error(response, ErrorCode::Conflict);
    }
    let response = agent.client().call(Request::CreateGroup { name: group("sudo") }, None).await.unwrap();
    assert_error(response, ErrorCode::Conflict);
    assert_eq!(agent.calls().lines().count(), 1, "nothing else ran");
}

#[tokio::test]
async fn passes_samba_passwords_on_stdin() {
    let agent = Agent::start(None, 0).await;
    let password = Password::new("correct horse battery").unwrap();
    let request = Request::SetSmbPassword { name: user("alice"), password };
    assert_eq!(agent.client().call(request, None).await.unwrap(), Response::Done);
    assert_eq!(agent.calls(), "smbpasswd -s -a alice\n");
    assert_eq!(agent.stdin(), "correct horse battery\ncorrect horse battery\n");
}

#[tokio::test]
async fn changes_managed_users_and_groups() {
    let agent = Agent::start(None, 0).await;
    let call = |request| async { agent.client().call(request, None).await.unwrap() };
    assert_eq!(call(Request::SetUserEnabled { name: user("alice"), enabled: false }).await, Response::Done);
    let members = Request::SetGroupMembers { group: group("family"), members: vec![user("alice")] };
    assert_eq!(call(members).await, Response::Done);
    let nobody = Request::SetGroupMembers { group: group("family"), members: vec![] };
    assert_eq!(call(nobody).await, Response::Done);
    assert_eq!(call(Request::CreateGroup { name: group("media") }).await, Response::Done);
    assert_eq!(call(Request::DeleteGroup { name: group("family") }).await, Response::Done);
    assert_eq!(call(Request::DeleteUser { name: user("alice") }).await, Response::Done);
    assert_eq!(
        agent.calls(),
        "smbpasswd -d alice
gpasswd --members alice -- family
gpasswd --members  -- family
groupadd --key GID_MIN=2000 --key GID_MAX=59999 -- media
groupdel -- family
smbpasswd -x alice
userdel -- alice
"
    );
}

#[tokio::test]
async fn never_touches_what_zendiloos_does_not_manage() {
    let agent = Agent::start(None, 0).await;
    let password = || Password::new("correct horse battery").unwrap();
    for request in [
        Request::DeleteUser { name: user("debian") },
        Request::DeleteUser { name: user("root") },
        Request::DeleteUser { name: user("zendiloos") },
        Request::DeleteUser { name: user("nobody") },
        Request::SetSmbPassword { name: user("debian"), password: password() },
        Request::SetUserEnabled { name: user("root"), enabled: false },
        Request::DeleteGroup { name: group("sudo") },
        Request::DeleteGroup { name: group("zendiloos-users") },
        Request::SetGroupMembers { group: group("sudo"), members: vec![user("alice")] },
        Request::SetGroupMembers { group: group("family"), members: vec![user("debian")] },
    ] {
        let name = request.name();
        let response = agent.client().call(request, None).await.unwrap();
        assert!(
            matches!(response, Response::Error { code: ErrorCode::Forbidden, .. }),
            "{name}: {response:?}"
        );
    }
    assert_eq!(agent.calls(), "", "nothing ran");
}
