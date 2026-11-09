//! The Docker engine against a fake `docker`: a script that records its
//! arguments and answers the few questions the agent asks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, HostPaths, Operations, Paths};
use zendiloos_agent_proto::{
    ComposeText, ContainerAction, ContainerId, ErrorCode, ExecCommand, NetworkName, ProjectAction,
    ProjectConfig, ProjectDirectory, ProjectName, Request, Response, Role, Subnet, TerminalSize,
};

const FIRST: &str = "d853eed323e74b49b6a84ea78784086ec720c3be10be36393589bfc80ad6ada1";
const SECOND: &str = "1c6c00e491b8aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Host {
    _dir: TempDir,
    root: PathBuf,
    agent: Operations,
}

impl Host {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let log = root.join("calls.log");
        let foreign = root.join("stack/compose.yaml");
        fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        fs::write(&foreign, "services:\n  web:\n    image: nginx\n").unwrap();
        let docker = root.join("docker");
        let script = format!(
            r#"#!/bin/sh
echo "docker $*" >> {log}
case "$*" in
  "ps --all --quiet --no-trunc") printf '%s\n%s\n' {FIRST} {SECOND} ;;
  "ps --all --quiet --no-trunc --filter label=com.docker.compose.project=empty") ;;
  "ps --all --quiet --no-trunc --filter "*) echo {FIRST} ;;
  "container inspect "*) echo '[{{"Id":"{FIRST}"}}]' ;;
  "top "*) printf 'PID PPID USER %%CPU %%MEM RSS ELAPSED COMMAND\n4242 4200 root 0.5 1.2 51200 01:02:03 nginx: master process\n' ;;
  "exec "*)
    [ -t 0 ] && echo "on a terminal: $*"
    read line
    echo "typed: $line" ;;
  "compose ls --all --format json")
    echo '[{{"Name":"stack","ConfigFiles":"{foreign}"}},{{"Name":"kuma","ConfigFiles":"/etc/zendiloos/apps/kuma.json"}},{{"Name":"shared","ConfigFiles":"/volume1/docker/shared/compose.yaml"}}]' ;;
  *" config --format json")
    if grep -q broken; then echo 'yaml: line 2: mapping values are not allowed' >&2; exit 15; fi
    echo '{{"name":"tesla"}}' ;;
  "logs "*) echo out; echo err >&2; echo again ;;
  "compose --project-name broken logs"*) echo 'no such project'; exit 1 ;;
  "network rm "*) ;;
  *" pull") echo ' web Pulling'; echo ' web Pulled' >&2 ;;
  *" up --detach --remove-orphans") printf ' Container tesla-web-1  Recreated\r Container tesla-web-1  Started\n' ;;
esac
"#,
            log = log.display(),
            foreign = foreign.display(),
        );
        fs::write(&docker, script).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        let any = root.join("tool");
        fs::write(&any, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&any, fs::Permissions::from_mode(0o755)).unwrap();
        let tools = Tools {
            systemd_mount: "/bin/false".into(),
            umount: "/bin/false".into(),
            rmdir: "/bin/false".into(),
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
            systemctl: any.clone(),
            busctl: any.clone(),
            timedatectl: any.clone(),
            sshd: any.clone(),
            systemd_run: any.clone(),
            docker,
            setsid: "/usr/bin/setsid".into(),
            openssl: "/bin/false".into(),
            nginx: "/bin/false".into(),
            rm: "/bin/false".into(),
            nft: "/bin/false".into(),
            dockerd: any,
        };
        let host = HostPaths {
            volumes: root.clone(),
            units: root.join("units"),
            projects: root.join("etc-projects"),
            project_shares: root.join("project-shares"),
            removable: root.join("removable"),
            ..HostPaths::default()
        };
        let paths = Paths { host, ..Paths::default() };
        let agent =
            Operations::with_paths(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused), paths)
                .unwrap();
        Self { _dir: dir, root, agent }
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    async fn output(&self, request: Request) -> String {
        match self.agent.execute(&request).await {
            Response::DockerOutput { text } => text,
            other => panic!("{other:?}"),
        }
    }
}

fn project(name: &str) -> ProjectName {
    ProjectName::parse(name).unwrap()
}

#[tokio::test]
async fn every_container_is_inspected_at_once() {
    let host = Host::new();

    let text = host.output(Request::DockerContainers {}).await;

    assert!(text.contains(FIRST), "{text}");
    assert_eq!(host.calls().last().unwrap(), &format!("docker container inspect {FIRST} {SECOND}"));
}

#[tokio::test]
async fn one_container_is_inspected_and_its_processes_listed() {
    let host = Host::new();
    let container = || ContainerId::parse(FIRST).unwrap();

    let text = host.output(Request::InspectContainer { container: container() }).await;
    assert!(text.contains(FIRST), "{text}");
    let text = host.output(Request::ContainerProcesses { container: container() }).await;
    assert!(text.contains("nginx: master process"), "{text}");
    assert_eq!(
        host.calls(),
        [
            format!("docker container inspect {FIRST}"),
            format!("docker top {FIRST} -eo pid,ppid,user,pcpu,pmem,rss,etime,args"),
        ]
    );
}

/// Reads from a terminal's master until `wanted` came, or it ended.
fn read_until(master: &std::os::fd::OwnedFd, wanted: &str) -> String {
    let mut seen = Vec::new();
    let mut buffer = [0u8; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !String::from_utf8_lossy(&seen).contains(wanted) {
        match rustix::io::read(master, &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen.extend_from_slice(&buffer[..n]),
        }
    }
    String::from_utf8_lossy(&seen).into_owned()
}

#[tokio::test]
async fn a_terminal_runs_docker_exec_on_its_own_and_hands_over_its_master() {
    let host = Host::new();
    let open = || Request::OpenTerminal {
        container: ContainerId::parse(FIRST).unwrap(),
        command: ExecCommand::from_line("/bin/sh -l").unwrap(),
        size: TerminalSize { columns: 120, rows: 40 },
    };

    let (answer, master) = host.agent.execute_with_file(&open()).await;
    assert_eq!(answer, Response::Terminal {});
    let master = master.expect("the terminal's master comes along");
    let seen = read_until(&master, "/bin/sh -l");
    assert!(
        seen.contains(&format!(
            "on a terminal: exec --interactive --tty --detach-keys=ctrl-@,ctrl-@,ctrl-@,ctrl-@ \
             --env TERM=xterm-256color {FIRST} /bin/sh -l"
        )),
        "{seen:?}"
    );
    rustix::io::write(&master, b"ls\n").unwrap();
    assert!(read_until(&master, "typed: ls").contains("typed: ls"));

    // With the first, sixteen at once; the next is refused until one hangs
    // up.
    let mut open_ones = vec![master];
    for _ in 0..15 {
        let (answer, master) = host.agent.execute_with_file(&open()).await;
        assert_eq!(answer, Response::Terminal {});
        open_ones.push(master.unwrap());
    }
    let (refused, none) = host.agent.execute_with_file(&open()).await;
    assert!(none.is_none());
    assert!(
        matches!(&refused, Response::Error { code: ErrorCode::Forbidden, message } if message.contains("16 terminals")),
        "{refused:?}"
    );
    drop(open_ones.pop());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (answer, _) = host.agent.execute_with_file(&open()).await;
        if matches!(answer, Response::Terminal {}) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "a terminal hung up is not counted any more");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn projects_start_by_their_containers_label() {
    let host = Host::new();

    let request = Request::ControlProject { project: project("stack"), action: ProjectAction::Restart };
    assert_eq!(host.agent.execute(&request).await, Response::Done);

    assert_eq!(
        host.calls(),
        [
            "docker ps --all --quiet --no-trunc --filter label=com.docker.compose.project=stack".to_owned(),
            format!("docker restart {FIRST}"),
        ]
    );
    let empty = Request::ControlProject { project: project("empty"), action: ProjectAction::Start };
    assert!(matches!(host.agent.execute(&empty).await, Response::Error { .. }));
}

#[tokio::test]
async fn foreign_projects_come_up_from_the_file_compose_reports() {
    let host = Host::new();
    let foreign = host.root.join("stack/compose.yaml").display().to_string();
    let directory = host.root.join("stack").display().to_string();

    let request = Request::ControlProject { project: project("stack"), action: ProjectAction::Up };
    assert_eq!(host.agent.execute(&request).await, Response::Done);

    assert_eq!(
        host.calls().last().unwrap(),
        &format!(
            "docker compose --file {foreign} --project-directory {directory} --project-name stack \
             up --detach --remove-orphans"
        )
    );
}

#[tokio::test]
async fn foreign_projects_start_stop_pull_and_go_down_as_compose_knows_them() {
    let host = Host::new();
    let foreign = host.root.join("stack/compose.yaml").display().to_string();
    let run = |action| Request::ControlProject { project: project("stack"), action };

    for action in [ProjectAction::Start, ProjectAction::Stop, ProjectAction::Pull, ProjectAction::Down] {
        assert_eq!(host.agent.execute(&run(action)).await, Response::Done, "{action:?}");
    }
    let calls = host.calls();
    assert!(calls.contains(&format!("docker start {FIRST}")), "{calls:?}");
    assert!(calls.contains(&format!("docker stop {FIRST}")));
    assert!(calls.iter().any(|call| call.contains(&format!("--file {foreign}")) && call.ends_with(" pull")));
    assert_eq!(calls.last().unwrap(), "docker compose --project-name stack down --remove-orphans");
    // Compose knows no file of it.
    let unknown = Request::ControlProject { project: project("ghost"), action: ProjectAction::Up };
    assert!(matches!(host.agent.execute(&unknown).await, Response::Error { .. }));
}

#[tokio::test]
async fn what_a_project_cannot_show_or_remove_is_reported() {
    let host = Host::new();
    let logs = Request::ProjectLogs { project: project("broken"), lines: 10 };
    let failed = host.agent.execute(&logs).await;
    assert!(
        matches!(&failed, Response::Error { message, .. } if message.contains("no such project")),
        "{failed:?}"
    );

    // A Compose file grown beyond what is read.
    fs::write(host.root.join("stack/compose.yaml"), "x".repeat(300 * 1024)).unwrap();
    let files = host.agent.execute(&Request::ProjectFiles { project: project("stack") }).await;
    assert!(
        matches!(&files, Response::Error { message, .. } if message.contains("at most 256 KiB")),
        "{files:?}"
    );

    // What deploying kept cannot be removed: reported, not left unsaid.
    fs::create_dir_all(host.root.join("etc-projects/tesla.json/inside")).unwrap();
    let down = Request::ControlProject { project: project("tesla"), action: ProjectAction::Down };
    assert!(matches!(host.agent.execute(&down).await, Response::Error { .. }));
}

#[tokio::test]
async fn foreign_projects_with_their_file_in_a_share_are_not_deployed() {
    let host = Host::new();

    for action in [ProjectAction::Up, ProjectAction::Pull] {
        let request = Request::ControlProject { project: project("shared"), action };
        let Response::Error { code, message } = host.agent.execute(&request).await else {
            panic!("{action:?} done")
        };
        assert_eq!(code, ErrorCode::Forbidden);
        assert!(message.contains("lies in a share"), "{message}");
    }
    assert!(!host.calls().iter().any(|call| call.contains(" up ") || call.contains(" pull")));
}

fn config(text: &str) -> ProjectConfig {
    ProjectConfig::parse(text).unwrap()
}

#[tokio::test]
async fn what_compose_writes_while_deploying_reaches_the_client_line_by_line() {
    use std::os::unix::fs::MetadataExt;
    use std::sync::Mutex;
    use zendiloos_agent::{Server, serve};
    use zendiloos_agent_proto::{AgentTransport, Report, UnixTransport};

    struct Heard(Mutex<Vec<String>>);
    impl Report for Heard {
        fn step(&self, _: &str, _: Option<u8>) {}
        fn line(&self, line: &str) {
            self.0.lock().unwrap().push(line.to_owned());
        }
    }

    let Host { _dir, root, agent } = Host::new();
    let socket = root.join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let our_uid = fs::metadata(&root).unwrap().uid();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(serve(listener, Server::new(agent, our_uid), async {
        let _ = stopped.await;
    }));
    let heard = Heard(Mutex::new(Vec::new()));
    let client = UnixTransport::new(&socket, std::time::Duration::from_secs(30));
    let confirmed = r#"{"name":"tesla","services":{"web":{"image":"nginx"}}}"#;
    let deploy = Request::DeployProject { project: project("tesla"), config: config(confirmed), pull: true };

    assert_eq!(client.call_reporting(deploy, None, &heard).await.unwrap(), Response::Done);
    assert_eq!(
        heard.0.into_inner().unwrap(),
        [
            " web Pulling",
            " web Pulled",
            " Container tesla-web-1  Recreated",
            " Container tesla-web-1  Started"
        ],
        "both streams, in order, before the answer"
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn administrators_projects_run_from_what_was_confirmed_and_kept() {
    let host = Host::new();
    let kept = host.root.join("etc-projects/tesla.json");
    let confirmed = r#"{"name":"tesla","services":{"web":{"image":"nginx","volumes":[
        {"type":"bind","source":"/etc/localtime","target":"/etc/localtime","read_only":true}]}}}"#;

    let deploy = Request::DeployProject { project: project("tesla"), config: config(confirmed), pull: true };
    assert_eq!(host.agent.execute(&deploy).await, Response::Done);

    let compose =
        format!("docker compose --file {} --env-file /dev/null --project-name tesla", kept.display());
    assert_eq!(host.calls(), [format!("{compose} pull"), format!("{compose} up --detach --remove-orphans")]);
    let file: serde_json::Value = serde_json::from_str(&fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(file["services"]["web"]["volumes"][0]["source"], "/etc/localtime");
    assert_eq!(file["x-zendiloos"]["pins"], serde_json::json!([]));
    assert_eq!(fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o600);

    // Down takes what it kept along.
    let down = Request::ControlProject { project: project("tesla"), action: ProjectAction::Down };
    assert_eq!(host.agent.execute(&down).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), &format!("{compose} down --remove-orphans"));
    assert!(!kept.exists());
}

#[tokio::test]
async fn a_bind_in_a_volume_that_cannot_be_pinned_stops_the_deploy() {
    let host = Host::new();
    fs::create_dir_all(host.root.join("volume1/docker/tesla/data")).unwrap();
    let source = host.root.join("volume1/docker/tesla/data").display().to_string();
    let confirmed = format!(
        r#"{{"services":{{"web":{{"image":"nginx","volumes":[{{"type":"bind","source":"{source}","target":"/data"}}]}}}}}}"#
    );

    let deploy =
        Request::DeployProject { project: project("tesla"), config: config(&confirmed), pull: false };
    let Response::Error { message, .. } = host.agent.execute(&deploy).await else { panic!("deployed") };

    assert!(message.contains("is not mounted"), "{message}");
    assert!(host.calls().is_empty(), "nothing ran: {:?}", host.calls());
    assert!(!host.root.join("etc-projects/tesla.json").exists());
}

#[tokio::test]
async fn apps_of_catalogs_stay_with_the_app_center() {
    let host = Host::new();

    for action in [ProjectAction::Up, ProjectAction::Pull, ProjectAction::Down] {
        let request = Request::ControlProject { project: project("kuma"), action };
        let Response::Error { code, .. } = host.agent.execute(&request).await else {
            panic!("{action:?} done")
        };
        assert_eq!(code, ErrorCode::Forbidden);
    }
    let files = host.agent.execute(&Request::ProjectFiles { project: project("kuma") }).await;
    assert!(
        matches!(files, Response::Error { code: ErrorCode::Forbidden, .. }),
        "secrets stay where they are"
    );
    assert!(!host.calls().iter().any(|call| call.contains(" up ") || call.contains(" down")));
}

#[tokio::test]
async fn foreign_compose_files_are_shown() {
    let host = Host::new();

    let text = host.output(Request::ProjectFiles { project: project("stack") }).await;

    let files: serde_json::Value = serde_json::from_str(&text).unwrap();
    let path = host.root.join("stack/compose.yaml").display().to_string();
    assert_eq!(files[path], "services:\n  web:\n    image: nginx\n");
}

#[tokio::test]
async fn compose_files_are_checked_from_stdin_and_errors_come_back() {
    let host = Host::new();
    let check = |compose: &str| Request::CheckCompose {
        project: project("tesla"),
        directory: ProjectDirectory::parse("/volume1/docker/tesla").unwrap(),
        compose: ComposeText::parse(compose).unwrap(),
    };

    assert_eq!(host.output(check("services: {}\n")).await, "{\"name\":\"tesla\"}\n");
    assert_eq!(
        host.calls().last().unwrap(),
        "docker compose --file - --project-directory /volume1/docker/tesla --project-name tesla --env-file /dev/null \
         config --format json"
    );
    let Response::Error { code, message } = host.agent.execute(&check("services:\n  broken: x: y\n")).await
    else {
        panic!("a broken file passed")
    };
    assert_eq!(code, ErrorCode::Forbidden);
    assert!(message.contains("mapping values are not allowed"), "{message}");
}

#[tokio::test]
async fn container_logs_keep_both_streams_in_order() {
    let host = Host::new();
    let container = ContainerId::parse("teslamate").unwrap();

    let text = host.output(Request::ContainerLogs { container, lines: 50 }).await;

    assert_eq!(text, "out\nerr\nagain\n");
    assert_eq!(host.calls().last().unwrap(), "docker logs --timestamps --tail 50 teslamate");
}

#[tokio::test]
async fn networks_of_dockers_own_and_gateways_without_subnet_are_refused() {
    let host = Host::new();

    let built_in = Request::RemoveNetwork { name: NetworkName::parse("bridge").unwrap() };
    assert!(matches!(host.agent.execute(&built_in).await, Response::Error { .. }));
    let gateway = Request::CreateNetwork {
        name: NetworkName::parse("proxy_net").unwrap(),
        subnet: None,
        gateway: Some("172.30.0.1".parse().unwrap()),
    };
    assert!(matches!(host.agent.execute(&gateway).await, Response::Error { .. }));
    assert!(host.calls().is_empty(), "docker was not asked");

    let create = Request::CreateNetwork {
        name: NetworkName::parse("proxy_net").unwrap(),
        subnet: Some(Subnet::parse("172.30.0.0/24").unwrap()),
        gateway: Some("172.30.0.1".parse().unwrap()),
    };
    assert_eq!(host.agent.execute(&create).await, Response::Done);
    assert_eq!(
        host.calls(),
        ["docker network create --driver bridge --subnet 172.30.0.0/24 --gateway 172.30.0.1 proxy_net"]
    );
    let stop = Request::ControlContainer {
        container: ContainerId::parse(FIRST).unwrap(),
        action: ContainerAction::Remove,
    };
    assert_eq!(host.agent.execute(&stop).await, Response::Done);
    assert_eq!(host.calls().last().unwrap(), &format!("docker rm {FIRST}"));
}
