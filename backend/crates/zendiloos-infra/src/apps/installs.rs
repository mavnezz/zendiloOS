//! What installing apps needs besides the agents: the shares (from the
//! Sharing context), the ports in use (from the kernel's socket tables)
//! and new secrets.

use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use zendiloos_apps::application::ports::{AppHostError, AppShares, ListeningPorts, Secrets, ShareChoice};
use zendiloos_kernel::{PortError, RequestContext};
use zendiloos_sharing::application::{SharingError, SharingUseCases};
use zendiloos_sharing::domain::{Access, ShareName};

/// [`AppShares`] through the Sharing context's use cases: the apps'
/// access is kept and written there, beside the people's.
#[derive(Clone)]
pub struct SharingForApps {
    sharing: SharingUseCases,
}

impl SharingForApps {
    pub fn new(sharing: SharingUseCases) -> Self {
        Self { sharing }
    }
}

fn sharing_error(error: SharingError) -> AppHostError {
    match error {
        SharingError::Refused(reason) => AppHostError::Refused(reason),
        SharingError::Port(error) => AppHostError::Port(error),
        other => AppHostError::Refused(other.to_string()),
    }
}

#[async_trait]
impl AppShares for SharingForApps {
    async fn shares(&self) -> Result<Vec<ShareChoice>, PortError> {
        let shares = self.sharing.list().await.map_err(|error| match error {
            SharingError::Port(error) => error,
            other => PortError::new("shares", other.to_string()),
        })?;
        Ok(shares
            .into_iter()
            .map(|share| ShareChoice { name: share.name.to_string(), volume: share.volume })
            .collect())
    }

    async fn grant(
        &self,
        account: &str,
        grants: &[(String, bool)],
        context: &RequestContext,
    ) -> Result<(), AppHostError> {
        let grants = grants
            .iter()
            .map(|(name, write)| {
                let name =
                    ShareName::parse(name).map_err(|error| AppHostError::Refused(error.to_string()))?;
                Ok((name, if *write { Access::ReadWrite } else { Access::Read }))
            })
            .collect::<Result<Vec<_>, AppHostError>>()?;
        self.sharing.set_app_access(account, &grants, context).await.map_err(sharing_error)
    }
}

/// The ports something listens on, from `/proc/net/{tcp,tcp6,udp,udp6}`:
/// TCP sockets listening and UDP sockets bound.
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
}

/// The local ports of a socket table; for TCP only those listening
/// (state `0A`).
fn ports(table: &str, tcp: bool) -> impl Iterator<Item = u16> + '_ {
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
impl ListeningPorts for ProcListening {
    async fn listening(&self) -> Result<BTreeSet<u16>, PortError> {
        let mut listening = BTreeSet::new();
        let mut read_any = false;
        for (file, tcp) in [("tcp", true), ("tcp6", true), ("udp", false), ("udp6", false)] {
            match tokio::fs::read_to_string(self.proc_net.join(file)).await {
                Ok(table) => {
                    read_any = true;
                    listening.extend(ports(&table, tcp));
                }
                // No IPv6 on this machine.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(PortError::new("socket tables", error)),
            }
        }
        if !read_any {
            return Err(PortError::new("socket tables", "none of them can be read"));
        }
        Ok(listening)
    }
}

/// Secrets from the operating system's random source.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomSecrets;

impl Secrets for RandomSecrets {
    fn generate(&self) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut secret = String::with_capacity(32);
        while secret.len() < 32 {
            let mut bytes = [0u8; 64];
            getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
            // 248 = 4 × 62: bytes above would favour some characters.
            for byte in bytes.into_iter().filter(|b| *b < 248) {
                if secret.len() == 32 {
                    break;
                }
                secret.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
            }
        }
        secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listening_ports_come_from_the_socket_tables() {
        let dir = tempfile::tempdir().unwrap();
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid\n";
        std::fs::write(
            dir.path().join("tcp"),
            format!(
                "{header}   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0\n\
                 1: 0100007F:0016 0100007F:D3A2 01 00000000:00000000 00:00000000 00000000 0\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("udp6"),
            format!("{header}   0: 00000000000000000000000000000000:14E9 00000000000000000000000000000000:0000 07 0\n"),
        )
        .unwrap();
        let listening = ProcListening::at(dir.path()).listening().await.unwrap();
        // 8080 listens, 22 is only a connection; 5353 is bound.
        assert_eq!(listening, BTreeSet::from([5353, 8080]));
        assert!(ProcListening::at(dir.path().join("none")).listening().await.is_err());
    }

    #[test]
    fn secrets_are_32_letters_and_digits() {
        let (one, two) = (RandomSecrets.generate(), RandomSecrets.generate());
        assert_eq!(one.len(), 32);
        assert!(one.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(one, two);
    }
}
