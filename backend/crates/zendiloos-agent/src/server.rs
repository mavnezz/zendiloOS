use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use zendiloos_agent_proto::{
    Envelope, ErrorCode, FrameError, MAX_REQUEST_BYTES, PROTOCOL_VERSION, Response, read_message,
    write_message,
};

use crate::operations::Operations;

/// A client must send its request within this time.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Operations running at once; further connections wait.
const MAX_CONCURRENT: usize = 4;

/// Accepts connections from one account and answers one request each.
#[derive(Debug, Clone)]
pub struct Server {
    operations: Arc<Operations>,
    client_uid: u32,
    permits: Arc<Semaphore>,
}

impl Server {
    pub fn new(operations: Operations, client_uid: u32) -> Self {
        Self {
            operations: Arc::new(operations),
            client_uid,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        }
    }

    async fn handle(&self, stream: UnixStream) {
        let peer = match stream.peer_cred() {
            Ok(peer) => peer,
            Err(error) => {
                tracing::warn!(%error, "cannot identify the peer; closing");
                return;
            }
        };
        let (uid, pid) = (peer.uid(), peer.pid());
        let (reader, mut writer) = stream.into_split();

        let response = if uid != self.client_uid {
            // The socket's permissions should already prevent this.
            tracing::warn!(uid, pid, "refused a connection from a foreign account");
            error_response(ErrorCode::Forbidden, "not the zendiloosd account")
        } else {
            self.respond(BufReader::new(reader), uid, pid).await
        };
        if let Err(error) = write_message(&mut writer, &response).await {
            tracing::debug!(%error, "cannot send the response");
        }
    }

    async fn respond(
        &self,
        mut reader: BufReader<impl tokio::io::AsyncRead + Unpin>,
        uid: u32,
        pid: Option<i32>,
    ) -> Response {
        let envelope: Envelope =
            match tokio::time::timeout(READ_TIMEOUT, read_message(&mut reader, MAX_REQUEST_BYTES)).await {
                Ok(Ok(envelope)) => envelope,
                Ok(Err(FrameError::Malformed(error))) => {
                    tracing::warn!(uid, pid, %error, "refused a malformed request");
                    return error_response(ErrorCode::InvalidRequest, "malformed request");
                }
                Ok(Err(error)) => {
                    tracing::warn!(uid, pid, %error, "refused an unreadable request");
                    return error_response(ErrorCode::InvalidRequest, "unreadable request");
                }
                Err(_) => return error_response(ErrorCode::InvalidRequest, "no request received in time"),
            };

        let request_id = envelope.request_id.as_ref().map(|id| id.as_str()).unwrap_or("-");
        let op = envelope.request.name();
        if envelope.version != PROTOCOL_VERSION {
            tracing::warn!(
                request_id,
                op,
                uid,
                pid,
                version = envelope.version,
                "refused another protocol version"
            );
            return error_response(
                ErrorCode::VersionMismatch,
                "zendiloosd and the agent are from different releases",
            );
        }

        let _permit = self.permits.acquire().await;
        let response = self.operations.execute(&envelope.request).await;
        match &response {
            Response::Error { code, message } => {
                tracing::warn!(request_id, op, uid, pid, ?code, message, "operation failed");
            }
            _ => tracing::info!(request_id, op, uid, pid, "operation done"),
        }
        response
    }
}

fn error_response(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

/// Serves connections until `shutdown` resolves.
pub async fn serve(listener: UnixListener, server: Server, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let server = server.clone();
                    tokio::spawn(async move { server.handle(stream).await });
                }
                Err(error) => {
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            () = &mut shutdown => break,
        }
    }
}
