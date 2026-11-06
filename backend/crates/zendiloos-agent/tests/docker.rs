//! The Docker engine against a fake `docker`: a script that records its
//! arguments and answers the few questions the agent asks.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use tempfile::TempDir;
use zendiloos_agent::config::Tools;
use zendiloos_agent::{Erasable, ErasePolicy, Operations};
use zendiloos_agent_proto::{
    ComposeText, ContainerAction, ContainerId, ErrorCode, NetworkName, ProjectAction, ProjectDirectory,
    ProjectFile, ProjectName, Request, Response, Role, Subnet,
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
  "compose ls --all --format json")
    echo '[{{"Name":"stack","ConfigFiles":"{foreign}"}},{{"Name":"kuma","ConfigFiles":"/etc/zendiloos/apps/kuma.json"}}]' ;;
  *" config --format json")
    if grep -q broken; then echo 'yaml: line 2: mapping values are not allowed' >&2; exit 15; fi
    echo '{{"name":"tesla"}}' ;;
  "logs "*) echo out; echo err >&2; echo again ;;
  "network rm "*) ;;
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
            systemctl: any.clone(),
            busctl: any.clone(),
            timedatectl: any.clone(),
            sshd: any.clone(),
            systemd_run: any.clone(),
            docker,
            dockerd: any,
        };
        let agent = Operations::new(Role::System, &tools, Erasable::Policy(ErasePolicy::Unused)).unwrap();
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
async fn projects_start_by_their_containers_label() {
    let host = Host::new();

    let request =
        Request::ControlProject { project: project("stack"), action: ProjectAction::Restart, file: None };
    assert_eq!(host.agent.execute(&request).await, Response::Done);

    assert_eq!(
        host.calls(),
        [
            "docker ps --all --quiet --no-trunc --filter label=com.docker.compose.project=stack".to_owned(),
            format!("docker restart {FIRST}"),
        ]
    );
    let empty =
        Request::ControlProject { project: project("empty"), action: ProjectAction::Start, file: None };
    assert!(matches!(host.agent.execute(&empty).await, Response::Error { .. }));
}

#[tokio::test]
async fn foreign_projects_come_up_from_the_file_compose_reports() {
    let host = Host::new();
    let foreign = host.root.join("stack/compose.yaml").display().to_string();
    let directory = host.root.join("stack").display().to_string();

    let request =
        Request::ControlProject { project: project("stack"), action: ProjectAction::Up, file: None };
    assert_eq!(host.agent.execute(&request).await, Response::Done);

    assert_eq!(
        host.calls().last().unwrap(),
        &format!(
            "docker compose --file {foreign} --project-directory {directory} --project-name stack \
             up --detach --remove-orphans --quiet-pull"
        )
    );
    let own = ProjectFile::parse("/volume1/docker/tesla/compose.yaml").unwrap();
    let pull =
        Request::ControlProject { project: project("tesla"), action: ProjectAction::Pull, file: Some(own) };
    assert_eq!(host.agent.execute(&pull).await, Response::Done);
    assert_eq!(
        host.calls().last().unwrap(),
        "docker compose --file /volume1/docker/tesla/compose.yaml --project-directory /volume1/docker/tesla \
         --project-name tesla pull --quiet"
    );
}

#[tokio::test]
async fn apps_of_catalogs_stay_with_the_app_center() {
    let host = Host::new();

    for action in [ProjectAction::Up, ProjectAction::Pull, ProjectAction::Down] {
        let request = Request::ControlProject { project: project("kuma"), action, file: None };
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
        "docker compose --file - --project-directory /volume1/docker/tesla --project-name tesla config --format json"
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
