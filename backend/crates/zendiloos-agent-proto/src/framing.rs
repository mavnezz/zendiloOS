//! One JSON object per line.

use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("connection closed before a complete message")]
    Closed,
    #[error("message larger than {limit} bytes")]
    TooLarge { limit: usize },
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub async fn write_message<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

/// Reads one message of at most `limit` bytes; stops reading beyond that.
pub async fn read_message<R, T>(reader: &mut R, limit: usize) -> Result<T, FrameError>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    let mut line = Vec::new();
    reader.take(limit as u64 + 1).read_until(b'\n', &mut line).await?;
    match line.pop() {
        Some(b'\n') if line.len() <= limit => Ok(serde_json::from_slice(&line)?),
        _ if line.len() >= limit => Err(FrameError::TooLarge { limit }),
        _ => Err(FrameError::Closed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Envelope, Request};

    async fn read(bytes: &[u8], limit: usize) -> Result<Envelope, FrameError> {
        let mut reader = bytes;
        read_message(&mut reader, limit).await
    }

    #[tokio::test]
    async fn round_trips_a_message() {
        let mut wire = Vec::new();
        write_message(&mut wire, &Envelope::new(Request::Hello {}, None)).await.unwrap();
        assert_eq!(wire.last(), Some(&b'\n'));
        assert_eq!(read(&wire, 1024).await.unwrap(), Envelope::new(Request::Hello {}, None));
    }

    #[tokio::test]
    async fn refuses_oversized_truncated_and_malformed_messages() {
        let line = b"{\"version\":1,\"request\":{\"op\":\"hello\"}}\n";
        assert!(read(line, line.len() - 1).await.is_ok(), "the limit excludes the newline");
        assert!(matches!(read(line, 10).await, Err(FrameError::TooLarge { limit: 10 })));
        assert!(matches!(read(&line[..20], 1024).await, Err(FrameError::Closed)));
        assert!(matches!(read(b"", 1024).await, Err(FrameError::Closed)));
        assert!(matches!(read(b"not json\n", 1024).await, Err(FrameError::Malformed(_))));
    }
}
