//! The clock against a fake host: `timedatectl` and `systemctl` replaced by
//! scripts that record their arguments, timesyncd's drop-in in a temporary
//! directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, Operations, Paths, TimePaths};
use zendiloos_agent_proto::{
    ClockTime, ErrorCode, Request, Response, Role, TimeServer, TimeServers, TimeZone,
};

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    /// `timedatectl` fails for `show-timesync` (timesyncd not running) and
    /// for the zone `Mars/Olympus`, as the real one does for unknown zones.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let log = root.join("calls.log");
        let script = |name: &str, body: &str| -> PathBuf {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let any = script("tool", "");
        let tools = Tools {
            timedatectl: script(
                "timedatectl",
                "case \"$*\" in\n\
                 show) printf 'Timezone=Etc/UTC\\nNTP=yes\\n' ;;\n\
                 show-timesync) exit 1 ;;\n\
                 list-timezones) printf 'Etc/UTC\\nEurope/Berlin\\n' ;;\n\
                 *Mars/Olympus*) echo 'Invalid or not installed time zone' >&2; exit 1 ;;\n\
                 esac",
            ),
            systemctl: script("systemctl", ""),
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
            busctl: any.clone(),
            sshd: any.clone(),
            systemd_run: any.clone(),
            docker: any.clone(),
            dockerd: any,
        };
        let paths = Paths {
            time: TimePaths { servers: root.join("timesyncd.conf.d/zendiloos.conf") },
            ..Paths::default()
        };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        Self { _dir: dir, root, agent }
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.lines().map(str::to_owned).collect()
    }

    fn servers(&self) -> Option<String> {
        fs::read_to_string(self.root.join("timesyncd.conf.d/zendiloos.conf")).ok()
    }
}

fn servers(hosts: &[&str]) -> TimeServers {
    TimeServers::new(hosts.iter().map(|host| TimeServer::parse(host).unwrap()).collect()).unwrap()
}

fn set(zone: &str, network_time: bool, hosts: &[&str], clock: Option<i64>) -> Request {
    Request::SetTime {
        zone: TimeZone::parse(zone).unwrap(),
        network_time,
        servers: servers(hosts),
        clock: clock.map(|seconds| ClockTime::from_unix_seconds(seconds).unwrap()),
    }
}

#[tokio::test]
async fn the_state_is_what_timedatectl_and_the_drop_in_say() {
    let host = Host::new();
    assert_eq!(host.agent.execute(&set("Europe/Berlin", true, &["ptb.de"], None)).await, Response::Done);

    let Response::TimeState { timedatectl, timesync, zones, servers_conf } =
        host.agent.execute(&Request::TimeState {}).await
    else {
        panic!("no state")
    };
    assert!(timedatectl.contains("Timezone=Etc/UTC"));
    assert_eq!(timesync, "", "timesyncd is not running");
    assert_eq!(zones.lines().count(), 2);
    assert!(servers_conf.contains("NTP=ptb.de\n"), "{servers_conf}");
}

#[tokio::test]
async fn network_time_writes_the_servers_and_restarts_timesyncd() {
    let host = Host::new();
    let request = set("Europe/Berlin", true, &["ptb.de", "192.168.1.1"], None);

    assert_eq!(host.agent.execute(&request).await, Response::Done);

    assert_eq!(
        host.calls(),
        [
            "timedatectl set-timezone -- Europe/Berlin",
            "timedatectl set-ntp true",
            "systemctl try-restart systemd-timesyncd.service",
        ]
    );
    let written = host.servers().unwrap();
    assert!(written.starts_with("# Written by zendiloOS"), "{written}");
    assert!(written.contains("[Time]\nNTP=ptb.de 192.168.1.1\n"), "{written}");
}

#[tokio::test]
async fn without_servers_the_drop_in_goes_and_the_clock_can_be_set() {
    let host = Host::new();
    host.agent.execute(&set("Etc/UTC", true, &["ptb.de"], None)).await;

    let response = host.agent.execute(&set("Etc/UTC", false, &[], Some(1_800_000_000))).await;

    assert_eq!(response, Response::Done);
    assert!(host.servers().is_none(), "Debian's pool again");
    assert_eq!(
        host.calls()[3..],
        [
            "timedatectl set-timezone -- Etc/UTC",
            "timedatectl set-ntp false",
            "timedatectl set-time @1800000000"
        ]
    );
}

#[tokio::test]
async fn the_clock_is_not_set_while_servers_keep_it() {
    let host = Host::new();

    let Response::Error { code, .. } =
        host.agent.execute(&set("Etc/UTC", true, &[], Some(1_800_000_000))).await
    else {
        panic!("set anyway")
    };

    assert_eq!(code, ErrorCode::Forbidden);
    assert!(host.calls().is_empty(), "nothing changed");
}

#[tokio::test]
async fn an_unknown_zone_changes_nothing_else() {
    let host = Host::new();

    let Response::Error { code, message } =
        host.agent.execute(&set("Mars/Olympus", true, &["ptb.de"], None)).await
    else {
        panic!("set anyway")
    };

    assert_eq!(code, ErrorCode::Failed);
    assert!(message.contains("Invalid or not installed time zone"), "{message}");
    assert!(host.servers().is_none());
    assert_eq!(host.calls(), ["timedatectl set-timezone -- Mars/Olympus"]);
}

#[test]
fn a_request_with_a_bad_value_is_not_read() {
    for request in [
        r#"{"op":"set_time","zone":"../etc/passwd","network_time":true,"servers":[]}"#,
        r#"{"op":"set_time","zone":"UTC","network_time":true,"servers":["a b"]}"#,
        r#"{"op":"set_time","zone":"UTC","network_time":false,"servers":[],"clock":0}"#,
    ] {
        assert!(serde_json::from_str::<Request>(request).is_err(), "{request}");
    }
}
