//! The two things zendiloosd needs from systemd, without linking libsystemd:
//! the socket it was started with, and telling it when startup is done.

use std::ffi::OsStr;
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};

use listenfd::ListenFd;

/// The Unix socket passed by socket activation (`zendiloosd.socket`), if any.
pub fn inherited_unix_listener() -> io::Result<Option<tokio::net::UnixListener>> {
    match ListenFd::from_env().take_unix_listener(0)? {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            Ok(Some(tokio::net::UnixListener::from_std(listener)?))
        }
        None => Ok(None),
    }
}

/// Reports readiness for `Type=notify`. Does nothing outside systemd.
pub fn notify_ready() {
    if let Some(target) = std::env::var_os("NOTIFY_SOCKET")
        && let Err(error) = notify(&target, "READY=1")
    {
        tracing::warn!(%error, "cannot notify systemd");
    }
}

/// Sends `state` to the notification socket `target`: a path, or an
/// abstract socket name starting with `@`.
fn notify(target: &OsStr, state: &str) -> io::Result<()> {
    let address = match target.as_bytes().strip_prefix(b"@") {
        Some(name) => SocketAddr::from_abstract_name(name)?,
        None => SocketAddr::from_pathname(target)?,
    };
    UnixDatagram::unbound()?.send_to_addr(state.as_bytes(), &address).map(|_| ())
}

/// Whether stderr goes to the journal, which adds its own timestamps.
pub fn logging_to_journal() -> bool {
    std::env::var_os("JOURNAL_STREAM").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receive(socket: &UnixDatagram) -> Vec<u8> {
        let mut buffer = [0; 64];
        let length = socket.recv(&mut buffer).unwrap();
        buffer[..length].to_vec()
    }

    #[test]
    fn notifies_a_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let receiver = UnixDatagram::bind(&path).unwrap();

        notify(path.as_os_str(), "READY=1").unwrap();

        assert_eq!(receive(&receiver), b"READY=1");
    }

    #[test]
    fn notifies_an_abstract_socket() {
        let name = format!("zendiloosd-test-{}", std::process::id());
        let receiver = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(&name).unwrap()).unwrap();

        notify(OsStr::new(&format!("@{name}")), "READY=1").unwrap();

        assert_eq!(receive(&receiver), b"READY=1");
    }
}
