//! The configuration backup's files and accounts against a fake host:
//! `systemctl`, `systemd-run`, `nginx` and the shadow tools replaced by
//! scripts that record their arguments (the shadow tools write the account
//! files as the real ones would), every place in a temporary directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{
    AccountFiles, ConfigurationPlaces, Erasable, ErasePolicy, NetworkPaths, Operations, Paths, SshPaths,
    TimePaths, WebPaths, WebServerPaths,
};
use zendiloos_agent_proto::{
    ConfigurationFile, ConfigurationPath, ErrorCode, GroupName, Request, Response, RestoredGroup,
    RestoredUser, Role, UserName,
};

const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n";
const GROUP: &str = "root:x:0:\nzendiloos-users:x:990:\n";
const PEM: &str = "-----BEGIN CERTIFICATE-----\nold\n-----END CERTIFICATE-----\n";
const KEY: &str = "-----BEGIN PRIVATE KEY-----\nold\n-----END PRIVATE KEY-----\n";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    /// `systemctl is-active` says smbd runs; `nginx -t` fails while
    /// `nginx-fails` is there; useradd and groupadd add their line.
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
        let at = |name: &str| root.join(name);
        let nginx = script(
            "nginx",
            &format!(
                "if [ -f {} ]; then echo 'nginx: [emerg] cannot load certificate'; exit 1; fi",
                at("nginx-fails").display()
            ),
        );
        // The last argument is the name; --uid/--gid the number.
        let add = |file: &str, line: &str| {
            format!(
                "id=; while [ $# -gt 1 ]; do case \"$1\" in --uid|--gid) id=$2; shift ;; esac; shift; done\n\
                 echo \"$1:x:$id:{line}\" >> {}",
                at(file).display()
            )
        };
        let tools = Tools {
            systemctl: script("systemctl", "case \"$*\" in 'is-active --quiet smbd.service') exit 0 ;; esac"),
            systemd_run: script(
                "systemd-run",
                &format!("while [ \"$1\" != {} ]; do shift; done\nexec \"$@\"", nginx.display()),
            ),
            nginx,
            useradd: script("useradd", &add("passwd", "990::/nonexistent:/usr/sbin/nologin")),
            groupadd: script("groupadd", &add("group", "")),
            gpasswd: script("gpasswd", ""),
            ..Tools::default()
        };
        fs::write(at("passwd"), PASSWD).unwrap();
        fs::write(at("group"), GROUP).unwrap();
        for folder in ["tls", "web/certificates/2", "network", "samba/private", "ssh"] {
            fs::create_dir_all(at(folder)).unwrap();
        }
        fs::write(at("tls/zendiloos.crt"), PEM).unwrap();
        fs::write(at("tls/zendiloos.key"), KEY).unwrap();
        fs::write(at("web/certificates/2/fullchain.pem"), PEM).unwrap();
        fs::write(at("web/certificates/2/key.pem"), KEY).unwrap();
        fs::write(
            at("web/https.conf"),
            format!("ssl_certificate     {};\n", at("tls/zendiloos.crt").display()),
        )
        .unwrap();
        fs::write(at("ssh/00-zendiloos.conf"), "Port 2222\n").unwrap();
        fs::write(at("samba/private/passdb.tdb"), b"TDB file\0\x01").unwrap();
        fs::write(at("hostid"), [0xa1, 0xb2, 0xc3, 0xd4]).unwrap();
        fs::write(at("network/05-zendiloos-enp1s0.network"), "[Match]\nName=enp1s0\n").unwrap();
        fs::write(at("network/10-other.network"), "[Match]\nName=eth9\n").unwrap();
        // A link where a file of the configuration belongs is not read.
        std::os::unix::fs::symlink(at("passwd"), at("id-high-water")).unwrap();
        let paths = Paths {
            accounts: AccountFiles {
                passwd: at("passwd"),
                group: at("group"),
                high_water: at("id-high-water"),
            },
            network: NetworkPaths {
                directory: at("network"),
                ifupdown: at("etc-network"),
                resolv_conf: at("resolv.conf"),
                internal: at("internal.nft"),
            },
            time: TimePaths { servers: at("timesyncd.conf") },
            ssh: SshPaths { config: at("ssh/00-zendiloos.conf") },
            web: WebPaths { tls: at("nginx-conf/tls.conf") },
            web_server: WebServerPaths {
                directory: at("web"),
                certificates: at("web/certificates"),
                own_certificate: at("tls/zendiloos.crt"),
                own_key: at("tls/zendiloos.key"),
                webroot: at("webroot"),
            },
            configuration: ConfigurationPlaces {
                samba_passwords: at("samba/private/passdb.tdb"),
                host_id: at("hostid"),
            },
            ..Paths::default()
        };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
        Self { _dir: dir, root, agent }
    }

    fn at(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.at("calls.log")).unwrap_or_default();
        log.lines().map(str::to_owned).collect()
    }

    fn text(&self, name: &str) -> String {
        fs::read_to_string(self.at(name)).unwrap()
    }
}

fn file(path: ConfigurationPath, content: &[u8]) -> ConfigurationFile {
    ConfigurationFile::new(path, content.to_vec()).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn a_backup_reads_the_files_of_the_configuration_that_are_there() {
    let host = Host::new();

    let response = host.agent.execute(&Request::ReadConfiguration {}).await;
    let Response::Configuration { files, users, groups } = response else { panic!("not read: {response:?}") };
    assert!(users.is_empty() && groups.is_empty(), "nobody's yet");

    let paths: Vec<String> = files.iter().map(|file| file.path.as_path()).collect();
    assert_eq!(
        paths,
        [
            "/etc/zendiloos/tls/zendiloos.crt",
            "/etc/zendiloos/tls/zendiloos.key",
            "/etc/ssh/sshd_config.d/00-zendiloos.conf",
            "/var/lib/samba/private/passdb.tdb",
            "/etc/hostid",
            "/etc/zendiloos/web/certificates/2/fullchain.pem",
            "/etc/zendiloos/web/certificates/2/key.pem",
            "/etc/systemd/network/05-zendiloos-enp1s0.network",
        ],
        "neither a link nor another's network file"
    );
    let passwords = files.iter().find(|file| file.path == ConfigurationPath::SambaPasswords).unwrap();
    assert_eq!(passwords.content.as_bytes(), b"TDB file\0\x01");
}

#[tokio::test]
async fn a_restore_writes_the_certificates_nginx_takes_and_samba_passwords_while_samba_is_stopped() {
    let host = Host::new();
    fs::remove_file(host.at("id-high-water")).unwrap();
    fs::write(host.at("id-high-water"), "uid=2005\n").unwrap();
    let new_pem = "-----BEGIN CERTIFICATE-----\nnew\n-----END CERTIFICATE-----\n";
    let new_key = "-----BEGIN PRIVATE KEY-----\nnew\n-----END PRIVATE KEY-----\n";
    let files = vec![
        file(ConfigurationPath::OwnCertificate, new_pem.as_bytes()),
        file(ConfigurationPath::OwnKey, new_key.as_bytes()),
        file(ConfigurationPath::IssuedCertificate(7), new_pem.as_bytes()),
        file(ConfigurationPath::IssuedKey(7), new_key.as_bytes()),
        file(ConfigurationPath::SambaPasswords, b"restored TDB"),
        file(ConfigurationPath::HostId, &[1, 2, 3, 4]),
        file(ConfigurationPath::IdHighWater, b"uid=2003\ngid=2004\n"),
    ];

    let response = host.agent.execute(&Request::RestoreConfiguration { files }).await;

    assert_eq!(response, Response::Done);
    assert_eq!(host.text("tls/zendiloos.crt"), new_pem);
    assert_eq!(host.text("web/certificates/7/key.pem"), new_key);
    assert_eq!(
        (mode(&host.at("tls/zendiloos.key")), mode(&host.at("web/certificates/7/fullchain.pem"))),
        (0o600, 0o644)
    );
    assert_eq!(host.text("samba/private/passdb.tdb"), "restored TDB");
    assert_eq!(mode(&host.at("samba/private/passdb.tdb")), 0o600);
    assert_eq!(fs::read(host.at("hostid")).unwrap(), [1, 2, 3, 4]);
    let marks = host.text("id-high-water");
    assert!(marks.contains("uid=2005\n") && marks.contains("gid=2004\n"), "{marks}");
    let calls = host.calls();
    let nginx = calls.iter().position(|call| call == "nginx -t -q").unwrap();
    assert_eq!(calls[nginx + 1], "systemctl try-reload-or-restart nginx.service");
    assert_eq!(
        calls[nginx + 2..],
        [
            "systemctl is-active --quiet smbd.service",
            "systemctl stop smbd.service",
            "systemctl start smbd.service"
        ]
    );
}

#[tokio::test]
async fn certificates_nginx_refuses_go_back_and_nothing_else_is_written() {
    let host = Host::new();
    fs::write(host.at("nginx-fails"), "").unwrap();
    let files = vec![
        file(ConfigurationPath::OwnCertificate, b"-----BEGIN CERTIFICATE-----\nbad\n"),
        file(ConfigurationPath::IssuedKey(9), b"-----BEGIN PRIVATE KEY-----\nbad\n"),
        file(ConfigurationPath::SambaPasswords, b"restored TDB"),
    ];

    let Response::Error { code, message } =
        host.agent.execute(&Request::RestoreConfiguration { files }).await
    else {
        panic!("restored anyway")
    };

    assert_eq!(code, ErrorCode::Failed);
    assert!(message.contains("cannot load certificate"), "{message}");
    assert_eq!(host.text("tls/zendiloos.crt"), PEM);
    assert!(!host.at("web/certificates/9/key.pem").exists(), "none before, none now");
    assert_eq!(fs::read(host.at("samba/private/passdb.tdb")).unwrap(), b"TDB file\0\x01");
}

#[tokio::test]
async fn files_written_from_settings_or_not_as_they_should_be_are_refused_before_anything_changes() {
    let host = Host::new();
    for bad in [
        file(ConfigurationPath::SshSettings, b"PermitRootLogin yes\n"),
        file(ConfigurationPath::Network("05-zendiloos-enp1s0.network".into()), b"[Match]\n"),
        file(ConfigurationPath::OwnKey, b"#!/bin/sh\n"),
        file(ConfigurationPath::IdHighWater, b"uid=2001\nsomething else\n"),
    ] {
        let files = vec![file(ConfigurationPath::HostId, &[9, 9, 9, 9]), bad];
        let response = host.agent.execute(&Request::RestoreConfiguration { files }).await;
        assert!(matches!(response, Response::Error { code: ErrorCode::Forbidden, .. }), "{response:?}");
    }
    assert_eq!(fs::read(host.at("hostid")).unwrap(), [0xa1, 0xb2, 0xc3, 0xd4]);
    assert!(host.calls().is_empty());
}

fn user(name: &str, uid: u32) -> RestoredUser {
    RestoredUser { name: UserName::parse(name).unwrap(), uid }
}

fn group(name: &str, gid: u32, members: &[&str]) -> RestoredGroup {
    RestoredGroup {
        name: GroupName::parse(name).unwrap(),
        gid,
        members: members.iter().map(|member| UserName::parse(member).unwrap()).collect(),
    }
}

#[tokio::test]
async fn accounts_come_back_with_their_ids_where_they_are_missing() {
    let host = Host::new();
    fs::remove_file(host.at("id-high-water")).unwrap();
    let existing = format!("{PASSWD}bob:x:2002:990::/nonexistent:/usr/sbin/nologin\n");
    fs::write(host.at("passwd"), existing).unwrap();
    let request = Request::RestoreAccounts {
        users: vec![user("anna", 2001), user("bob", 2002), user("carl", 2007)],
        groups: vec![group("family", 2003, &["anna", "bob"])],
    };

    assert_eq!(host.agent.execute(&request).await, Response::Done);

    let calls = host.calls();
    assert_eq!(calls.iter().filter(|call| call.starts_with("useradd")).count(), 2, "bob is there");
    assert!(calls.iter().any(|call| call.ends_with("--uid 2001 -- anna")), "{calls:?}");
    assert!(calls.contains(&"groupadd --gid 2003 -- family".to_owned()));
    assert_eq!(calls.last().unwrap(), "gpasswd --members anna,bob -- family");
    let marks = host.text("id-high-water");
    assert!(marks.contains("uid=2007") && marks.contains("gid=2003"), "{marks}");
    // Again: all there, nothing made.
    assert_eq!(host.agent.execute(&request).await, Response::Done);
    // Read for a backup as they are now (gpasswd here writes no members).
    let Response::Configuration { users, groups, .. } =
        host.agent.execute(&Request::ReadConfiguration {}).await
    else {
        panic!("not read")
    };
    let ids: Vec<(String, u32)> =
        users.iter().map(|user| (user.name.as_str().to_owned(), user.uid)).collect();
    assert_eq!(ids, [("bob".to_owned(), 2002), ("anna".to_owned(), 2001), ("carl".to_owned(), 2007)]);
    assert_eq!((groups[0].name.as_str(), groups[0].gid), ("family", 2003));
    assert_eq!(host.calls().iter().filter(|call| call.starts_with("useradd")).count(), 2);
}

#[tokio::test]
async fn an_id_or_name_taken_otherwise_refuses_the_whole_restore() {
    let host = Host::new();
    let taken = format!("{PASSWD}eve:x:2001:990::/nonexistent:/usr/sbin/nologin\n");
    fs::write(host.at("passwd"), taken).unwrap();
    for request in [
        Request::RestoreAccounts { users: vec![user("carl", 2009), user("anna", 2001)], groups: vec![] },
        Request::RestoreAccounts { users: vec![user("eve", 2004)], groups: vec![] },
        Request::RestoreAccounts { users: vec![], groups: vec![group("family", 2003, &["nobody-here"])] },
        Request::RestoreAccounts { users: vec![], groups: vec![group("staff", 990, &[])] },
    ] {
        let response = host.agent.execute(&request).await;
        assert!(matches!(response, Response::Error { code: ErrorCode::Forbidden, .. }), "{response:?}");
    }
    assert!(host.calls().is_empty(), "nothing made");
}
