use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use std::os::fd::OwnedFd;

use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use zendiloos_agent_proto::{
    Envelope, ErrorCode, FrameError, MAX_REQUEST_BYTES, PROTOCOL_VERSION, Request, Response, read_message,
    write_message_with_file,
};

use crate::operations::Operations;

/// A client must send its request within this time.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Quick operations running at once; further connections wait.
const MAX_CONCURRENT: usize = 4;
/// Operations that walk whole trees or disks, which may take hours, running
/// at once. They have places of their own, so one user's large copies never
/// keep everyone else from listing, downloading or uploading.
const MAX_CONCURRENT_LONG: usize = 2;

/// Whether `request` may take long: copying, moving or deleting trees,
/// permissions of a whole share, preparing or destroying pools, fetching
/// and running apps.
fn is_long(request: &Request) -> bool {
    match request {
        Request::CopyEntry { .. }
        | Request::MoveEntry { .. }
        | Request::RemoveEntry { .. }
        // Planning walks the whole folder.
        | Request::ArchiveFolder { .. }
        // Restarts Docker.
        | Request::EnableDocker { .. }
        // Fetch images, start or stop containers.
        | Request::PrepareApp { .. }
        | Request::PullApp { .. }
        | Request::StartApp { .. }
        | Request::StopApp { .. }
        | Request::RemoveApp { .. }
        | Request::DeployProject { .. }
        | Request::CreatePool { .. }
        | Request::DestroyPool { .. }
        | Request::ReplaceDisk { .. }
        | Request::DestroyShare { .. } => true,
        Request::SetShareAccess { recursive, .. } => *recursive,
        _ => false,
    }
}

/// Accepts connections from one account and answers one request each.
#[derive(Debug, Clone)]
pub struct Server {
    operations: Arc<Operations>,
    client_uid: u32,
    permits: Arc<Semaphore>,
    long_permits: Arc<Semaphore>,
}

impl Server {
    pub fn new(operations: Operations, client_uid: u32) -> Self {
        Self {
            operations: Arc::new(operations),
            client_uid,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            long_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_LONG)),
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
        let (reader, writer) = stream.into_split();

        let (response, file) = if uid != self.client_uid {
            // The socket's permissions should already prevent this.
            tracing::warn!(uid, pid, "refused a connection from a foreign account");
            (error_response(ErrorCode::Forbidden, "not the zendiloosd account"), None)
        } else {
            self.respond(BufReader::new(reader), uid, pid).await
        };
        if let Err(error) = write_message_with_file(writer.as_ref(), &response, file.as_ref()).await {
            tracing::debug!(%error, "cannot send the response");
        }
    }

    async fn respond(
        &self,
        mut reader: BufReader<impl tokio::io::AsyncRead + Unpin>,
        uid: u32,
        pid: Option<i32>,
    ) -> (Response, Option<OwnedFd>) {
        let envelope: Envelope =
            match tokio::time::timeout(READ_TIMEOUT, read_message(&mut reader, MAX_REQUEST_BYTES)).await {
                Ok(Ok(envelope)) => envelope,
                Ok(Err(FrameError::Malformed(error))) => {
                    tracing::warn!(uid, pid, %error, "refused a malformed request");
                    return (error_response(ErrorCode::InvalidRequest, "malformed request"), None);
                }
                Ok(Err(error)) => {
                    tracing::warn!(uid, pid, %error, "refused an unreadable request");
                    return (error_response(ErrorCode::InvalidRequest, "unreadable request"), None);
                }
                Err(_) => {
                    return (error_response(ErrorCode::InvalidRequest, "no request received in time"), None);
                }
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
            let message = "zendiloosd and the agent are from different releases";
            return (error_response(ErrorCode::VersionMismatch, message), None);
        }

        let permits = if is_long(&envelope.request) { &self.long_permits } else { &self.permits };
        let _permit = permits.acquire().await;
        let (response, file) = self.operations.execute_with_file(&envelope.request).await;
        match &response {
            Response::Error { code, message } => {
                tracing::warn!(request_id, op, uid, pid, ?code, message, "operation failed");
            }
            _ => tracing::info!(request_id, op, uid, pid, "operation done"),
        }
        (response, file)
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

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{FileLocation, PoolName, ShareName, SharePath, UserName, VolumeNumber};

    use super::*;

    #[test]
    fn tree_walks_wait_in_a_queue_of_their_own() {
        let user = || UserName::parse("anna").unwrap();
        let volume = VolumeNumber::new(1).unwrap();
        let share = ShareName::parse("photos").unwrap();
        let location = FileLocation {
            volume,
            root: zendiloos_agent_proto::FileRoot::Share(share.clone()),
            path: SharePath::parse("album").unwrap(),
            snapshot: None,
        };
        assert!(!is_long(&Request::ListDirectory { user: user(), location: location.clone() }));
        assert!(!is_long(&Request::ScrubPool { pool: PoolName::parse("tank").unwrap() }));
        assert!(is_long(&Request::RemoveEntry { user: user(), location, recycle: true }));
        let access = |recursive| Request::SetShareAccess {
            volume,
            name: share.clone(),
            access: vec![],
            removed: vec![],
            recursive,
        };
        assert!(is_long(&access(true)));
        assert!(!is_long(&access(false)));
    }
}
