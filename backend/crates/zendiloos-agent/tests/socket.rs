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
use zendiloos_agent::{Operations, Server, serve};
use zendiloos_agent_proto::{
    AgentTransport, ErrorCode, PROTOCOL_VERSION, Request, RequestId, Response, UnixTransport,
};

struct Agent {
    dir: TempDir,
    stop: Option<oneshot::Sender<()>>,
}

impl Agent {
    /// Starts an agent serving `client_uid`; `systemctl` is a script that
    /// appends its arguments to `calls.log` and exits with `exit_code`.
    async fn start(client_uid: Option<u32>, exit_code: i32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let systemctl = dir.path().join("systemctl");
        let log = dir.path().join("calls.log");
        fs::write(&systemctl, format!("#!/bin/sh\necho \"$@\" >> {}\nexit {exit_code}\n", log.display()))
            .unwrap();
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();

        // Files we create belong to us: the test runs as the client.
        let our_uid = fs::metadata(dir.path()).unwrap().uid();
        let operations = Operations::new(&Tools { systemctl }).unwrap();
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
    assert_eq!(agent.calls(), "reboot\npoweroff\n");
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
