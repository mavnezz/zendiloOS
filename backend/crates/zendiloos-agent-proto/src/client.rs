//! The unprivileged side: how `zendiloosd` reaches the agent.

use std::io;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::UnixStream;

use crate::descriptor::{read_message_with_file, write_message_with_file};
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
    #[error("the agent did not answer within {0:?}")]
    Timeout(Duration),
    #[error("talking to the agent failed")]
    Frame(#[from] FrameError),
    #[error("talking to the agent failed")]
    Io(#[from] io::Error),
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
}

/// The agent's Unix socket.
#[derive(Debug, Clone)]
pub struct UnixTransport {
    socket: PathBuf,
    timeout: Duration,
}

impl UnixTransport {
    pub fn new(socket: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self { socket: socket.into(), timeout }
    }

    async fn exchange(&self, envelope: &Envelope) -> Result<(Response, Option<OwnedFd>), TransportError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|source| TransportError::Connect { path: self.socket.clone(), source })?;
        write_message_with_file(&stream, envelope, None).await?;
        Ok(read_message_with_file(&stream, MAX_RESPONSE_BYTES).await?)
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
        let envelope = Envelope::new(request, request_id);
        tokio::time::timeout(self.timeout, self.exchange(&envelope))
            .await
            .map_err(|_| TransportError::Timeout(self.timeout))?
    }
}
