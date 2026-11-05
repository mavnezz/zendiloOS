//! The unprivileged side: how `zendiloosd` reaches the agent.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::BufReader;
use tokio::net::UnixStream;

use crate::framing::{FrameError, read_message, write_message};
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

    async fn exchange(&self, envelope: &Envelope) -> Result<Response, TransportError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|source| TransportError::Connect { path: self.socket.clone(), source })?;
        let (reader, mut writer) = stream.into_split();
        write_message(&mut writer, envelope).await?;
        Ok(read_message(&mut BufReader::new(reader), MAX_RESPONSE_BYTES).await?)
    }
}

#[async_trait]
impl AgentTransport for UnixTransport {
    async fn call(
        &self,
        request: Request,
        request_id: Option<RequestId>,
    ) -> Result<Response, TransportError> {
        let envelope = Envelope::new(request, request_id);
        tokio::time::timeout(self.timeout, self.exchange(&envelope))
            .await
            .map_err(|_| TransportError::Timeout(self.timeout))?
    }
}
