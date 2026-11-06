//! The ports something on this machine listens on, from the socket tables
//! in `/proc/net`: TCP sockets listening and UDP sockets bound.

use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use zendiloos_apps::application::ports::ListeningPorts as AppPorts;
use zendiloos_kernel::PortError;
use zendiloos_system::application::ports::ListeningPorts as SystemPorts;

const WHAT: &str = "socket tables";

/// For the apps (every port in use) and for SSH (TCP listeners).
#[derive(Debug, Clone)]
pub struct ProcListening {
    proc_net: PathBuf,
}

impl Default for ProcListening {
    fn default() -> Self {
        Self { proc_net: PathBuf::from("/proc/net") }
    }
}

impl ProcListening {
    /// Reads the tables in `proc_net` instead. For tests.
    pub fn at(proc_net: impl Into<PathBuf>) -> Self {
        Self { proc_net: proc_net.into() }
    }

    /// The ports of these tables; an error if none can be read.
    async fn ports(&self, tables: &[(&str, bool)]) -> Result<BTreeSet<u16>, PortError> {
        let mut ports = BTreeSet::new();
        let mut read_any = false;
        for (file, tcp) in tables {
            match tokio::fs::read_to_string(self.proc_net.join(file)).await {
                Ok(table) => {
                    read_any = true;
                    ports.extend(local_ports(&table, *tcp));
                }
                // No IPv6 on this machine.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(PortError::new(WHAT, error)),
            }
        }
        if !read_any {
            return Err(PortError::new(WHAT, "none of them can be read"));
        }
        Ok(ports)
    }
}

/// The local ports of a socket table; for TCP only those listening
/// (state `0A`).
fn local_ports(table: &str, tcp: bool) -> impl Iterator<Item = u16> + '_ {
    table.lines().skip(1).filter_map(move |line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (local, state) = (fields.get(1)?, fields.get(3)?);
        if tcp && *state != "0A" {
            return None;
        }
        let (_, port) = local.rsplit_once(':')?;
        u16::from_str_radix(port, 16).ok().filter(|port| *port != 0)
    })
}

#[async_trait]
impl AppPorts for ProcListening {
    async fn listening(&self) -> Result<BTreeSet<u16>, PortError> {
        self.ports(&[("tcp", true), ("tcp6", true), ("udp", false), ("udp6", false)]).await
    }
}

#[async_trait]
impl SystemPorts for ProcListening {
    async fn is_listening(&self, port: u16) -> Result<bool, PortError> {
        Ok(self.ports(&[("tcp", true), ("tcp6", true)]).await?.contains(&port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid\n";

    fn tables() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tcp"),
            format!(
                "{HEADER}   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0\n\
                 1: 0100007F:0016 0100007F:D3A2 01 00000000:00000000 00:00000000 00000000 0\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tcp6"),
            format!("{HEADER}   0: 00000000000000000000000000000000:08AE 00000000000000000000000000000000:0000 0A 0\n"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("udp6"),
            format!("{HEADER}   0: 00000000000000000000000000000000:14E9 00000000000000000000000000000000:0000 07 0\n"),
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn apps_see_every_port_in_use() {
        let dir = tables();
        let listening = AppPorts::listening(&ProcListening::at(dir.path())).await.unwrap();
        // 8080 and 2222 listen, 22 is only a connection; 5353 is bound.
        assert_eq!(listening, BTreeSet::from([2222, 5353, 8080]));
        assert!(AppPorts::listening(&ProcListening::at(dir.path().join("none"))).await.is_err());
    }

    #[tokio::test]
    async fn ssh_sees_tcp_listeners_only() {
        let dir = tables();
        let ports = ProcListening::at(dir.path());
        assert!(ports.is_listening(8080).await.unwrap());
        assert!(ports.is_listening(2222).await.unwrap(), "IPv6");
        assert!(!ports.is_listening(22).await.unwrap(), "a connection, not a listener");
        assert!(!ports.is_listening(5353).await.unwrap(), "UDP");
    }
}
