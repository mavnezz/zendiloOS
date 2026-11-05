//! Messages that carry an open file: the descriptor travels as SCM_RIGHTS
//! ancillary data attached to the first byte of the JSON line. Only the
//! files agent sends descriptors, and only of files it opened as the user
//! the request names.

use std::io::{self, IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};

use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::Interest;
use tokio::net::UnixStream;

use crate::framing::FrameError;

/// Sends `message` as one line, with `file` attached if given.
pub async fn write_message_with_file<T: Serialize>(
    stream: &UnixStream,
    message: &T,
    file: Option<&OwnedFd>,
) -> io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut sent = 0;
    if let Some(file) = file {
        sent = stream
            .async_io(Interest::WRITABLE, || {
                let files = [file.as_fd()];
                let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
                let mut control = SendAncillaryBuffer::new(&mut space);
                if !control.push(SendAncillaryMessage::ScmRights(&files)) {
                    return Err(io::Error::other("no room for the descriptor"));
                }
                Ok(sendmsg(stream, &[IoSlice::new(&line)], &mut control, SendFlags::NOSIGNAL)?)
            })
            .await?;
    }
    while sent < line.len() {
        sent += stream.async_io(Interest::WRITABLE, || stream.try_write(&line[sent..])).await?;
    }
    Ok(())
}

/// Reads one line of at most `limit` bytes, and the descriptor that came
/// with it, if any.
pub async fn read_message_with_file<T: DeserializeOwned>(
    stream: &UnixStream,
    limit: usize,
) -> Result<(T, Option<OwnedFd>), FrameError> {
    let mut line = Vec::new();
    let mut file = None;
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let (read, files, truncated) = stream
            .async_io(Interest::READABLE, || {
                let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
                let mut control = RecvAncillaryBuffer::new(&mut space);
                let message = recvmsg(
                    stream,
                    &mut [IoSliceMut::new(&mut chunk)],
                    &mut control,
                    RecvFlags::CMSG_CLOEXEC,
                )?;
                let mut files = Vec::new();
                for item in control.drain() {
                    if let RecvAncillaryMessage::ScmRights(received) = item {
                        files.extend(received);
                    }
                }
                Ok((message.bytes, files, message.flags.contains(ReturnFlags::CTRUNC)))
            })
            .await?;
        if truncated || files.len() > 1 || (file.is_some() && !files.is_empty()) {
            return Err(FrameError::Io(io::Error::other("unexpected descriptors in the answer")));
        }
        file = file.or(files.into_iter().next());
        if read == 0 {
            return Err(FrameError::Closed);
        }
        let data = &chunk[..read];
        if let Some(end) = data.iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&data[..end]);
            if line.len() > limit {
                return Err(FrameError::TooLarge { limit });
            }
            return Ok((serde_json::from_slice(&line)?, file));
        }
        line.extend_from_slice(data);
        if line.len() > limit {
            return Err(FrameError::TooLarge { limit });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, Write};

    use super::*;
    use crate::Response;

    #[tokio::test]
    async fn passes_a_file_with_the_message() {
        let (left, right) = UnixStream::pair().unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"contents").unwrap();
        file.rewind().unwrap();
        let descriptor = OwnedFd::from(file);

        write_message_with_file(&left, &Response::File { size: 8 }, Some(&descriptor)).await.unwrap();
        let (message, received): (Response, _) = read_message_with_file(&right, 1024).await.unwrap();

        assert_eq!(message, Response::File { size: 8 });
        let mut text = String::new();
        std::fs::File::from(received.unwrap()).read_to_string(&mut text).unwrap();
        assert_eq!(text, "contents");
    }

    #[tokio::test]
    async fn reads_long_messages_without_a_file() {
        let (left, right) = UnixStream::pair().unwrap();
        let long = Response::Error { code: crate::ErrorCode::Failed, message: "x".repeat(200_000) };
        let writer = tokio::spawn(async move { write_message_with_file(&left, &long, None).await });
        let (message, file): (Response, _) = read_message_with_file(&right, 1 << 20).await.unwrap();
        writer.await.unwrap().unwrap();
        assert!(matches!(message, Response::Error { message, .. } if message.len() == 200_000));
        assert!(file.is_none());

        let (left, right) = UnixStream::pair().unwrap();
        write_message_with_file(&left, &Response::Done, None).await.unwrap();
        assert!(matches!(
            read_message_with_file::<Response>(&right, 3).await,
            Err(FrameError::TooLarge { limit: 3 })
        ));
    }
}
