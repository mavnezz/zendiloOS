//! The SSH server against a fake host: `systemctl`, `sshd` and
//! `systemd-run` replaced by scripts that record their arguments, the
//! settings file in a temporary directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, Operations, Paths, SshPaths};
use zendiloos_agent_proto::{ErrorCode, OffAfter, Request, Response, Role, RootLogin, SshConfig, SshPort};

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    /// `sshd -t` refuses a file with port 4444, as the real one refuses a
    /// broken configuration.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let log = root.join("calls.log");
        let config = root.join("sshd_config.d/00-zendiloos.conf");
        let script = |name: &str, body: &str| -> PathBuf {
            let path = root.join(name);
            fs::write(&path, format!("#!/bin/sh\necho \"{name} $*\" >> {}\n{body}\n", log.display()))
                .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let any = script("tool", "");
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
            systemctl: script(
                "systemctl",
                "case \"$1\" in\n\
                 is-enabled) echo enabled ;;\n\
                 is-active) echo active ;;\n\
                 show) printf 'SubState=waiting\\nDescription=zendiloOS turns SSH off at 1800000600\\n' ;;\n\
                 esac",
            ),
            sshd: script("sshd", &format!("! grep -q 'Port 4444' {}", config.display())),
            // `--wait` runs the command, as the transient unit would.
            systemd_run: script(
                "systemd-run",
                "case \"$1\" in --wait) while [ \"${1#--}\" != \"$1\" ]; do shift; done; exec \"$@\" ;; esac",
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
            smbcontrol: any.clone(),
            useradd: any.clone(),
            userdel: any.clone(),
            groupadd: any.clone(),
            groupdel: any.clone(),
            gpasswd: any.clone(),
            smbpasswd: any.clone(),
            busctl: any.clone(),
            ip: "/bin/false".into(),
            timedatectl: any.clone(),
            docker: any.clone(),
            setsid: "/usr/bin/setsid".into(),
            dockerd: any,
        };
        let paths = Paths { ssh: SshPaths { config }, ..Paths::default() };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        Self { _dir: dir, root, agent }
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.lines().map(str::to_owned).collect()
    }

    /// The check `sshd -t` in its transient unit, as logged.
    fn check(&self) -> [String; 2] {
        let sshd = self.root.join("sshd").display().to_string();
        [
            format!(
                "systemd-run --wait --pipe --collect --quiet --property=RuntimeDirectory=sshd \
                 --property=RuntimeDirectoryPreserve=yes {sshd} -t"
            ),
            "sshd -t".to_owned(),
        ]
    }

    fn config(&self) -> Option<String> {
        fs::read_to_string(self.root.join("sshd_config.d/00-zendiloos.conf")).ok()
    }
}

fn config(enabled: bool, port: u16, off_after: Option<u16>) -> SshConfig {
    SshConfig {
        enabled,
        port: SshPort::new(port).unwrap(),
        root_login: Some(RootLogin::No),
        password_login: None,
        off_after: off_after.map(|minutes| OffAfter::minutes(minutes).unwrap()),
    }
}

fn set(config: SshConfig) -> Request {
    Request::SetSsh { config }
}

#[tokio::test]
async fn ssh_runs_on_its_port_and_a_timer_turns_it_off() {
    let host = Host::new();

    assert_eq!(host.agent.execute(&set(config(true, 2222, Some(10)))).await, Response::Done);

    let written = host.config().unwrap();
    assert!(written.contains("Port 2222\nPermitRootLogin no\n"), "{written}");
    assert!(!written.contains("PasswordAuthentication"), "as the system has it: {written}");
    let systemctl = host.root.join("systemctl").display().to_string();
    let [systemd_run, sshd] = host.check();
    let calls = host.calls();
    let (timer, before) = calls.split_last().unwrap();
    assert_eq!(
        before,
        [
            systemd_run,
            sshd,
            "systemctl stop zendiloos-ssh-off.timer".to_owned(),
            "systemctl reset-failed zendiloos-ssh-off.service".to_owned(),
            // Enabled until the next boot only: the timer does not survive
            // a restart, so neither may the start with the machine.
            "systemctl disable --no-reload ssh.service".to_owned(),
            "systemctl enable --no-reload --runtime --now ssh.service".to_owned(),
            "systemctl show --property=NeedDaemonReload --value ssh.service".to_owned(),
            "systemctl reload-or-restart ssh.service".to_owned(),
        ]
    );
    // The time it fires rides along in the description.
    let (at, rest) = timer
        .strip_prefix("systemd-run --unit=zendiloos-ssh-off --description=zendiloOS turns SSH off at ")
        .and_then(|rest| rest.split_once(' '))
        .unwrap_or_else(|| panic!("{timer}"));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    assert!((now + 590..=now + 610).contains(&at.parse::<u64>().unwrap()), "{at}");
    assert_eq!(
        rest,
        format!(
            "--on-active=10min --timer-property=AccuracySec=1s --collect --setenv=SYSTEMCTL_SKIP_SYSV=1 \
             {systemctl} disable --runtime --now ssh.service"
        )
    );
    let Response::SshState { installed, enabled, active, config, off_at } =
        host.agent.execute(&Request::SshState {}).await
    else {
        panic!("no state")
    };
    assert!(installed);
    assert_eq!((enabled.as_str(), active.as_str(), off_at), ("enabled", "active", Some(1_800_000_600)));
    assert!(config.contains("Port 2222"));
}

#[tokio::test]
async fn turned_off_it_stays_off_and_no_timer_is_left() {
    let host = Host::new();

    assert_eq!(host.agent.execute(&set(config(false, 22, None))).await, Response::Done);

    let [systemd_run, sshd] = host.check();
    assert_eq!(
        host.calls(),
        [
            systemd_run,
            sshd,
            "systemctl stop zendiloos-ssh-off.timer".to_owned(),
            "systemctl reset-failed zendiloos-ssh-off.service".to_owned(),
            "systemctl disable --no-reload --now ssh.service".to_owned(),
            "systemctl disable --no-reload --runtime ssh.service".to_owned(),
            "systemctl show --property=NeedDaemonReload --value ssh.service".to_owned(),
        ]
    );
}

#[tokio::test]
async fn turned_on_for_good_it_starts_with_the_machine() {
    let host = Host::new();

    assert_eq!(host.agent.execute(&set(config(true, 22, None))).await, Response::Done);

    let [systemd_run, sshd] = host.check();
    assert_eq!(
        host.calls(),
        [
            systemd_run,
            sshd,
            "systemctl stop zendiloos-ssh-off.timer".to_owned(),
            "systemctl reset-failed zendiloos-ssh-off.service".to_owned(),
            "systemctl disable --no-reload --runtime ssh.service".to_owned(),
            "systemctl enable --no-reload --now ssh.service".to_owned(),
            "systemctl show --property=NeedDaemonReload --value ssh.service".to_owned(),
            "systemctl reload-or-restart ssh.service".to_owned(),
        ],
        "no timer"
    );
}

#[tokio::test]
async fn settings_sshd_refuses_give_way_to_the_previous_ones() {
    let host = Host::new();
    host.agent.execute(&set(config(true, 2222, None))).await;
    let before = host.config().unwrap();

    let Response::Error { code, .. } = host.agent.execute(&set(config(true, 4444, None))).await else {
        panic!("applied anyway")
    };

    assert_eq!(code, ErrorCode::Failed);
    assert_eq!(host.config().unwrap(), before, "the previous file is back");
    assert_eq!(host.calls().last().unwrap(), "sshd -t", "nothing restarted");
}

#[tokio::test]
async fn a_first_file_sshd_refuses_goes_again() {
    let host = Host::new();
    let refused = host.agent.execute(&set(config(true, 4444, None))).await;
    assert!(matches!(refused, Response::Error { code: ErrorCode::Failed, .. }), "{refused:?}");
    assert!(host.config().is_none(), "no file was there, none stays");
}

#[tokio::test]
async fn without_the_ssh_server_nothing_is_written() {
    let host = Host::new();
    fs::remove_file(host.root.join("sshd")).unwrap();

    let Response::SshState { installed, .. } = host.agent.execute(&Request::SshState {}).await else {
        panic!("no state")
    };
    assert!(!installed);
    let refused = host.agent.execute(&set(config(true, 2222, None))).await;
    assert!(
        matches!(&refused, Response::Error { message, .. } if message.contains("not installed")),
        "{refused:?}"
    );
    assert!(host.config().is_none());
}

#[tokio::test]
async fn a_settings_file_that_cannot_be_read_is_reported() {
    let host = Host::new();
    fs::create_dir_all(host.root.join("sshd_config.d/00-zendiloos.conf/inside")).unwrap();
    let state = host.agent.execute(&Request::SshState {}).await;
    assert!(matches!(state, Response::Error { code: ErrorCode::Failed, .. }), "{state:?}");
    let set = host.agent.execute(&set(config(true, 2222, None))).await;
    assert!(matches!(set, Response::Error { code: ErrorCode::Failed, .. }), "{set:?}");
}

#[test]
fn a_port_a_service_of_the_nas_uses_is_not_read() {
    let request = r#"{"op":"set_ssh","config":{"enabled":true,"port":445,"root_login":null,"password_login":null,"off_after":null}}"#;
    assert!(serde_json::from_str::<Request>(request).is_err());
}
