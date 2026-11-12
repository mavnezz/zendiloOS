use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use std::os::fd::OwnedFd;

use tokio::io::{AsyncReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Semaphore, watch};
use zendiloos_agent_proto::{
    Envelope, ErrorCode, FrameError, MAX_REQUEST_BYTES, PROTOCOL_VERSION, Response, read_message,
    write_message_with_file,
};

use crate::operations::Operations;
use crate::waiting::Turn;

/// A client must send its request within this time.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Quick operations running at once; further connections wait.
const MAX_CONCURRENT: usize = 4;
/// Operations that walk whole trees or disks, which may take hours, running
/// at once. They have places of their own, so one user's large copies never
/// keep everyone else from listing, downloading or uploading.
const MAX_CONCURRENT_LONG: usize = 2;
/// Changes of whole shares' permissions running at once, beside the long
/// ones.
const MAX_CONCURRENT_WALKS: usize = 2;
/// Disks read from at once to make them blink, beside everything else;
/// more wait for their turn.
const MAX_CONCURRENT_BURSTS: usize = 2;
/// Backups and restores of volumes running at once, beside the long
/// operations: each takes up to hours in a unit of its own. zendiloosd runs
/// one at a time; the second place is for ending one or removing an old
/// archive meanwhile.
const MAX_CONCURRENT_VOLUME_WORK: usize = 2;
/// Backup tasks' runs, restores and retention at once, beside everything
/// else: each up to a day in a unit of its own, one per task.
const MAX_CONCURRENT_BACKUP_WORK: usize = 4;

/// Accepts connections from one account and answers one request each.
#[derive(Debug, Clone)]
pub struct Server {
    operations: Arc<Operations>,
    client_uid: u32,
    permits: Arc<Semaphore>,
    long_permits: Arc<Semaphore>,
    walk_permits: Arc<Semaphore>,
    burst_permits: Arc<Semaphore>,
    volume_permits: Arc<Semaphore>,
    backup_permits: Arc<Semaphore>,
    /// Counting the volumes' data, one at a time.
    count_permit: Arc<Semaphore>,
    /// One long place per user at a time.
    users: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<Semaphore>>>>,
    /// Set once the agent is asked to stop: what has not started yet does
    /// not start any more.
    stopping: Arc<watch::Sender<bool>>,
}

impl Server {
    pub fn new(operations: Operations, client_uid: u32) -> Self {
        Self {
            operations: Arc::new(operations),
            client_uid,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            long_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_LONG)),
            walk_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_WALKS)),
            burst_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_BURSTS)),
            volume_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_VOLUME_WORK)),
            backup_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_BACKUP_WORK)),
            count_permit: Arc::new(Semaphore::new(1)),
            users: Arc::default(),
            stopping: Arc::new(watch::channel(false).0),
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
            self.respond(BufReader::new(reader), writer.as_ref(), uid, pid).await
        };
        if let Err(error) = write_message_with_file(writer.as_ref(), &response, file.as_ref()).await {
            tracing::debug!(%error, "cannot send the response");
        }
    }

    async fn respond(
        &self,
        mut reader: BufReader<impl tokio::io::AsyncRead + Unpin>,
        writer: &UnixStream,
        uid: u32,
        pid: Option<i32>,
    ) -> (Response, Option<OwnedFd>) {
        let received = tokio::time::Instant::now();
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
        let reads = envelope.request.only_reads();
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

        let permits = if envelope.request.is_walk() {
            &self.walk_permits
        } else if envelope.request.is_burst() {
            &self.burst_permits
        } else if envelope.request.is_volume_work() {
            &self.volume_permits
        } else if envelope.request.is_backup_work() {
            &self.backup_permits
        } else if envelope.request.is_count() {
            &self.count_permit
        } else if envelope.request.is_long() {
            &self.long_permits
        } else {
            &self.permits
        };
        let own = envelope.request.acting_user().map(|user| self.user_place(user.as_str()));
        // The request is read whole, so the connection says nothing more
        // until the client closes it: it gave up (zendiloosd's own timeout)
        // and reported a failure. While the request waits for a place or a
        // lock, that ends the wait (see `waiting`); once it runs, it is
        // seen through, except by work that stops when it hears so (a
        // search, a volume's unit).
        let (gone_tx, gone) = watch::channel(false);
        let until = envelope.wait_ms.map(|ms| received + Duration::from_millis(ms) / 2);
        let turn = Turn::new(gone, self.stopping.subscribe(), until);
        let (steps_out, mut steps) = tokio::sync::mpsc::unbounded_channel();
        let work = turn.scope(crate::progress::scope(steps_out, async {
            // The user's own place first: a second long request of theirs
            // waits behind their first, not in a place of everybody's.
            let _own = match &own {
                Some(place) => Some(crate::waiting::wait(place.acquire()).await?),
                None => None,
            };
            let _permit = crate::waiting::wait(permits.acquire()).await?;
            Ok::<_, crate::operations::OperationError>(
                self.operations.execute_with_file(&envelope.request).await,
            )
        }));
        tokio::pin!(work);
        let mut more = [0u8; 1];
        let mut watching = true;
        let outcome = loop {
            tokio::select! {
                outcome = &mut work => break outcome,
                _ = reader.read(&mut more), if watching => {
                    watching = false;
                    let _ = gone_tx.send(true);
                }
                // Steps and lines go out as they come; a client that hung up
                // hears none.
                Some(note) = steps.recv() => {
                    if watching {
                        let _ = write_message_with_file(writer, &note.into_response(), None).await;
                    }
                }
            }
        };
        // What came just before the end still goes out before the answer:
        // the last lines are often what matters ("… Started").
        while let Ok(note) = steps.try_recv() {
            if watching {
                let _ = write_message_with_file(writer, &note.into_response(), None).await;
            }
        }
        let (response, file) = match outcome {
            Ok(answer) => answer,
            Err(error) => {
                tracing::warn!(request_id, op, uid, pid, %error, "not carried out");
                return (error_response(ErrorCode::Failed, &error.to_string()), None);
            }
        };
        match &response {
            Response::Error { code, message } => {
                tracing::warn!(request_id, op, uid, pid, ?code, message, "operation failed");
            }
            // Looking, asked for many times a minute, would fill the journal.
            _ if reads => tracing::debug!(request_id, op, uid, pid, "read"),
            _ => tracing::info!(request_id, op, uid, pid, "operation done"),
        }
        (response, file)
    }

    fn user_place(&self, user: &str) -> Arc<Semaphore> {
        let mut users = self.users.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(users.entry(user.to_owned()).or_insert_with(|| Arc::new(Semaphore::new(1))))
    }

    /// Nothing that has not started yet starts any more.
    fn stop(&self) {
        let _ = self.stopping.send(true);
    }
}

fn error_response(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

/// Serves connections until `shutdown` resolves.
pub async fn serve(listener: UnixListener, server: Server, shutdown: impl Future<Output = ()>) {
    tokio::pin!(shutdown);
    let mut running = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let server = server.clone();
                    running.spawn(async move { server.handle(stream).await });
                }
                Err(error) => {
                    tracing::warn!(%error, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            Some(_) = running.join_next(), if !running.is_empty() => {}
            () = &mut shutdown => break,
        }
    }
    // Asked to stop (a package upgrade, a restart): nothing new is taken,
    // what waits for its turn is refused, and what runs is seen through, or
    // a pool's change or a copy of Docker's data would stop halfway.
    // systemd's TimeoutStopSec bounds it.
    drop(listener);
    server.stop();
    if !running.is_empty() {
        tracing::info!(running = running.len(), "stopping once the running operations are done");
    }
    while running.join_next().await.is_some() {}
}
