//! Renaming the machine against a fake host: `hostnamectl`, `systemctl`,
//! `openssl`, `systemd-run` and `nginx` replaced by scripts that record
//! their arguments; `/etc/hostname`, `/etc/hosts` and the web server's
//! files in a temporary directory.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, HostNamePaths, Operations, Paths, WebServerPaths};
use zendiloos_agent_proto::{ErrorCode, HostName, Request, Response, Role};

const HOSTS: &str =
    "127.0.0.1\tlocalhost\n127.0.1.1\tzendiloos\n\n::1     localhost ip6-localhost ip6-loopback\n";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    /// `hostnamectl` refuses the name `refused`; `systemctl` knows no
    /// wsdd2; `openssl` writes what it was asked for into its two files;
    /// `nginx -t` fails while `nginx-fails` is there. nginx serves the own
    /// certificate unless `issued`.
    fn new(issued: bool) -> Self {
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
        let nginx = script(
            "nginx",
            &format!(
                "if [ -f {} ]; then echo 'nginx: [emerg] cannot load certificate'; exit 1; fi",
                root.join("nginx-fails").display()
            ),
        );
        let tools = Tools {
            hostnamectl: script(
                "hostnamectl",
                "case \"$*\" in *refused*) echo 'Invalid hostname' >&2; exit 1 ;; esac",
            ),
            systemctl: script(
                "systemctl",
                "case \"$*\" in *wsdd2*) echo 'Unit wsdd2.service not found.' >&2; exit 5 ;; esac",
            ),
            openssl: script(
                "openssl",
                "while [ $# -gt 0 ]; do case \"$1\" in\n\
                 -keyout) echo \"KEY $subject\" > \"$2\"; shift ;;\n\
                 -out) echo \"CERTIFICATE $subject\" > \"$2\"; shift ;;\n\
                 -subj) subject=\"$2\"; shift ;;\n\
                 esac; shift; done",
            ),
            systemd_run: script(
                "systemd-run",
                &format!("while [ \"$1\" != {} ]; do shift; done\nexec \"$@\"", nginx.display()),
            ),
            nginx,
            ..Tools::default()
        };
        fs::write(root.join("hostname"), "zendiloos\n").unwrap();
        fs::write(root.join("hosts"), HOSTS).unwrap();
        fs::create_dir_all(root.join("tls")).unwrap();
        fs::write(root.join("tls/zendiloos.crt"), "CERTIFICATE /CN=zendiloos\n").unwrap();
        fs::write(root.join("tls/zendiloos.key"), "KEY /CN=zendiloos\n").unwrap();
        fs::create_dir_all(root.join("web")).unwrap();
        let served = if issued {
            "/etc/zendiloos/web/certificates/1/fullchain.pem".to_owned()
        } else {
            root.join("tls/zendiloos.crt").display().to_string()
        };
        fs::write(
            root.join("web/https.conf"),
            format!("listen 443 ssl default_server;\nssl_certificate     {served};\n"),
        )
        .unwrap();
        let paths = Paths {
            host_name: HostNamePaths { hostname: root.join("hostname"), hosts: root.join("hosts") },
            web_server: WebServerPaths {
                directory: root.join("web"),
                certificates: root.join("web/certificates"),
                own_certificate: root.join("tls/zendiloos.crt"),
                own_key: root.join("tls/zendiloos.key"),
                webroot: root.join("webroot"),
            },
            ..Paths::default()
        };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::None), paths).unwrap();
        Self { _dir: dir, root, agent }
    }

    fn calls(&self) -> Vec<String> {
        let log = fs::read_to_string(self.root.join("calls.log")).unwrap_or_default();
        log.lines().map(str::to_owned).collect()
    }

    fn file(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name)).unwrap()
    }

    async fn rename(&self, name: &str) -> Response {
        self.agent.execute(&Request::SetHostName { name: HostName::parse(name).unwrap() }).await
    }
}

#[tokio::test]
async fn the_state_is_what_etc_hostname_holds() {
    let host = Host::new(false);
    assert_eq!(
        host.agent.execute(&Request::HostNameState {}).await,
        Response::HostNameState { hostname: "zendiloos\n".into() }
    );
    fs::remove_file(host.root.join("hostname")).unwrap();
    assert_eq!(
        host.agent.execute(&Request::HostNameState {}).await,
        Response::HostNameState { hostname: String::new() }
    );
}

#[tokio::test]
async fn a_rename_reaches_hostnamed_the_hosts_file_the_certificate_and_what_follows_the_name() {
    let host = Host::new(false);

    let response = host.rename("Office").await;

    assert_eq!(response, Response::HostNameSet { certificate_renewed: true, own_certificate_served: true });
    let calls = host.calls();
    assert_eq!(calls[0], "hostnamectl hostname -- office");
    assert!(calls[1].starts_with("openssl req -x509 -newkey ec"), "{calls:?}");
    assert!(
        calls[1].contains(
            "-subj /CN=office -addext subjectAltName=DNS:office,DNS:office.local,DNS:localhost,IP:127.0.0.1"
        ),
        "{calls:?}"
    );
    assert_eq!(
        calls[2..],
        [
            "systemd-run --quiet --wait --pipe --collect --service-type=exec --description=zendiloOS: checking nginx's settings --property=PrivateNetwork=yes --property=PrivateTmp=yes"
                .to_owned()
                + &format!(" {} -t -q", host.root.join("nginx").display()),
            "nginx -t -q".to_owned(),
            "systemctl try-reload-or-restart nginx.service".into(),
            "systemctl try-restart avahi-daemon.service".into(),
            "systemctl try-restart smbd.service".into(),
            "systemctl try-restart wsdd2.service".into(),
        ]
    );
    assert_eq!(host.file("hosts"), HOSTS.replace("127.0.1.1\tzendiloos", "127.0.1.1\toffice"));
    assert_eq!(host.file("tls/zendiloos.crt"), "CERTIFICATE /CN=office\n");
    assert_eq!(host.file("tls/zendiloos.key"), "KEY /CN=office\n");
    let mode = |name: &str| fs::metadata(host.root.join(name)).unwrap().permissions().mode() & 0o777;
    assert_eq!((mode("tls/zendiloos.key"), mode("tls/zendiloos.crt")), (0o600, 0o644));
    assert!(!host.root.join("tls/zendiloos-new").exists(), "nothing left beside the pair");
}

#[tokio::test]
async fn an_issued_certificate_is_never_touched_and_nginx_is_left_alone() {
    let host = Host::new(true);

    let response = host.rename("office").await;

    assert_eq!(response, Response::HostNameSet { certificate_renewed: true, own_certificate_served: false });
    assert!(host.calls().iter().all(|call| !call.contains("nginx")), "{:?}", host.calls());
    // The own one names the new name, for when it is served again.
    assert_eq!(host.file("tls/zendiloos.crt"), "CERTIFICATE /CN=office\n");
}

#[tokio::test]
async fn a_certificate_nginx_refuses_leaves_the_pair_before_and_the_rename_stands() {
    let host = Host::new(false);
    fs::write(host.root.join("nginx-fails"), "").unwrap();

    let response = host.rename("office").await;

    assert_eq!(response, Response::HostNameSet { certificate_renewed: false, own_certificate_served: false });
    assert_eq!(host.file("tls/zendiloos.crt"), "CERTIFICATE /CN=zendiloos\n");
    assert_eq!(host.file("tls/zendiloos.key"), "KEY /CN=zendiloos\n");
    assert!(host.file("hosts").contains("127.0.1.1\toffice\n"));
    assert!(host.calls().iter().any(|call| call == "systemctl try-restart smbd.service"));
}

#[tokio::test]
async fn a_name_hostnamed_refuses_changes_nothing() {
    let host = Host::new(false);

    let Response::Error { code, message } = host.rename("refused").await else { panic!("renamed anyway") };

    assert_eq!(code, ErrorCode::Failed);
    assert!(message.contains("Invalid hostname"), "{message}");
    assert_eq!(host.calls(), ["hostnamectl hostname -- refused"]);
    assert_eq!(host.file("hosts"), HOSTS);
    assert_eq!(host.file("tls/zendiloos.crt"), "CERTIFICATE /CN=zendiloos\n");
}

#[tokio::test]
async fn a_hosts_file_that_is_a_link_is_left_alone() {
    let host = Host::new(false);
    fs::remove_file(host.root.join("hosts")).unwrap();
    fs::write(host.root.join("elsewhere"), HOSTS).unwrap();
    std::os::unix::fs::symlink(host.root.join("elsewhere"), host.root.join("hosts")).unwrap();

    assert!(matches!(host.rename("office").await, Response::HostNameSet { .. }));
    assert_eq!(host.file("elsewhere"), HOSTS);
}

#[test]
fn a_request_with_a_bad_name_is_not_read() {
    for request in [
        r#"{"op":"set_host_name","name":"nas.local"}"#,
        r#"{"op":"set_host_name","name":"-nas"}"#,
        r#"{"op":"set_host_name","name":"1234"}"#,
        r#"{"op":"set_host_name","name":"nas","force":true}"#,
    ] {
        assert!(serde_json::from_str::<Request>(request).is_err(), "{request}");
    }
}
