//! The unprivileged side: how `zendiloosd` reaches the agent.

use std::io;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::UnixStream;

use crate::descriptor::{MessageReader, write_message_with_file};
use crate::framing::FrameError;
use crate::{Envelope, MAX_RESPONSE_BYTES, Request, RequestId, Response};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("cannot connect to the agent at {}", path.display())]
    Connect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the agent did not answer within {0:?}; what was asked may still be going on")]
    Timeout(Duration),
    #[error("talking to the agent failed")]
    Frame(#[from] FrameError),
    #[error("talking to the agent failed")]
    Io(#[from] io::Error),
}

/// Where the steps the agent reports on the way go.
pub trait Report: Send + Sync {
    fn step(&self, step: &str, percent: Option<u8>);

    /// A line a program the agent runs wrote ([`Response::Output`]);
    /// dropped unless asked for.
    fn line(&self, line: &str) {
        let _ = line;
    }
}

impl<F: Fn(&str, Option<u8>) + Send + Sync> Report for F {
    fn step(&self, step: &str, percent: Option<u8>) {
        self(step, percent);
    }
}

/// Hears nothing.
struct Silent;

impl Report for Silent {
    fn step(&self, _: &str, _: Option<u8>) {}
}

/// A way to send requests to the agent: the real socket, or a simulation.
#[async_trait]
pub trait AgentTransport: Send + Sync {
    async fn call(&self, request: Request, request_id: Option<RequestId>)
    -> Result<Response, TransportError>;

    /// Like [`AgentTransport::call`], for requests whose answer carries an
    /// open file.
    async fn call_with_file(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<(Response, Option<OwnedFd>), TransportError> {
        Ok((self.call(request, request_id).await?, None))
    }

    /// Like [`AgentTransport::call`], telling `report` each step the agent
    /// reports on the way ([`Response::Progress`]).
    async fn call_reporting(
        &self,
        request: Request,
        request_id: Option<RequestId>,
        report: &dyn Report,
    ) -> Result<Response, TransportError> {
        let _ = report;
        self.call(request, request_id).await
    }
}

/// The agent's Unix socket.
#[derive(Debug, Clone)]
pub struct UnixTransport {
    socket: PathBuf,
    timeout: Duration,
    /// For requests that may take long ([`Request::is_long`]).
    long: Option<Duration>,
}

impl UnixTransport {
    /// Waits `timeout` for an answer. A request still waiting for its turn
    /// when that runs out is dropped by the agent, never carried out later;
    /// one that started is finished.
    pub fn new(socket: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self { socket: socket.into(), timeout, long: None }
    }

    /// Waits `long` for requests that may take long.
    pub fn with_long(self, long: Duration) -> Self {
        Self { long: Some(long), ..self }
    }

    /// Sends `envelope` and reads the answer; progress reports before it go
    /// to `report`.
    async fn exchange(
        &self,
        envelope: &Envelope,
        report: &dyn Report,
    ) -> Result<(Response, Option<OwnedFd>), TransportError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|source| TransportError::Connect { path: self.socket.clone(), source })?;
        write_message_with_file(&stream, envelope, None).await?;
        let mut reader = MessageReader::new(&stream);
        let mut file = None;
        loop {
            let (response, received) = reader.next(MAX_RESPONSE_BYTES).await?;
            file = file.or(received);
            match response {
                Response::Progress { step, percent } => report.step(&step, percent),
                Response::Output { line } => report.line(&line),
                answer => return Ok((answer, file)),
            }
        }
    }

    async fn timed(
        &self,
        request: Request,
        request_id: Option<RequestId>,
        report: &dyn Report,
    ) -> Result<(Response, Option<OwnedFd>), TransportError> {
        let limit = if request.is_long() { self.long.unwrap_or(self.timeout) } else { self.timeout };
        let envelope = Envelope::new(request, request_id).waiting(limit);
        tokio::time::timeout(limit, self.exchange(&envelope, report))
            .await
            .map_err(|_| TransportError::Timeout(limit))?
    }
}

#[async_trait]
impl AgentTransport for UnixTransport {
    async fn call(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<Response, TransportError> {
        // A descriptor nobody asked for is closed here.
        Ok(self.call_with_file(request, request_id).await?.0)
    }

    async fn call_with_file(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<(Response, Option<OwnedFd>), TransportError> {
        self.timed(request, request_id, &Silent).await
    }

    async fn call_reporting(
        &self,
        request: Request,
        request_id: Option<RequestId>,
        report: &dyn Report,
    ) -> Result<Response, TransportError> {
        // A descriptor nobody asked for is closed here.
        Ok(self.timed(request, request_id, report).await?.0)
    }
}
