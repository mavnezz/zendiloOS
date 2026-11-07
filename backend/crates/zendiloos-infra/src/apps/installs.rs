//! What the App Center needs besides the agents: the shares (from the
//! Sharing context), the ports in use (from the kernel's socket tables)
//! and new secrets.

use async_trait::async_trait;
use zendiloos_apps::application::ports::{AppHostError, AppShares, Secrets, UsableShare, UsableShares};
use zendiloos_apps::domain::SecretAlphabet;
use zendiloos_kernel::{Actor, PortError, RequestContext};
use zendiloos_sharing::application::{FilesError, FilesUseCases, SharingError, SharingUseCases};
use zendiloos_sharing::domain::{Access, ShareName};

/// [`UsableShares`] through the file manager's use cases: the shares as
/// the administrator may use them, which an app made from a template does
/// as them.
#[derive(Clone)]
pub struct FilesForApps {
    files: FilesUseCases,
}

impl FilesForApps {
    pub fn new(files: FilesUseCases) -> Self {
        Self { files }
    }
}

#[async_trait]
impl UsableShares for FilesForApps {
    async fn usable(&self, actor: &Actor) -> Result<Vec<UsableShare>, PortError> {
        let shares = self.files.shares(actor).await.map_err(|error| match error {
            FilesError::Port(error) => error,
            other => PortError::new("shares", other.to_string()),
        })?;
        Ok(shares
            .into_iter()
            .map(|usable| UsableShare {
                name: usable.share.name.to_string(),
                volume: usable.share.volume,
                writable: usable.access == Access::ReadWrite,
            })
            .collect())
    }
}

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

/// Secrets from the operating system's random source.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomSecrets;

impl Secrets for RandomSecrets {
    fn generate(&self, length: u8, alphabet: SecretAlphabet) -> String {
        let alphabet: &[u8] = match alphabet {
            SecretAlphabet::Alphanumeric => b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
            SecretAlphabet::Hex => b"0123456789abcdef",
        };
        // Bytes at or above the largest multiple of the alphabet's size
        // would favour some characters: they are thrown away.
        let limit = 256 - 256 % alphabet.len();
        let length = usize::from(length);
        let mut secret = String::with_capacity(length);
        while secret.len() < length {
            let mut bytes = [0u8; 64];
            getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
            for byte in bytes.into_iter().filter(|b| usize::from(*b) < limit) {
                if secret.len() == length {
                    break;
                }
                secret.push(char::from(alphabet[usize::from(byte) % alphabet.len()]));
            }
        }
        secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_have_the_length_and_characters_asked_for() {
        let generate = |length, alphabet| RandomSecrets.generate(length, alphabet);
        let (one, two) =
            (generate(32, SecretAlphabet::Alphanumeric), generate(32, SecretAlphabet::Alphanumeric));
        assert_eq!(one.len(), 32);
        assert!(one.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(one, two);
        let hex = generate(64, SecretAlphabet::Hex);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }
}
