//! What installing apps needs besides the agents: the shares (from the
//! Sharing context), the ports in use (from the kernel's socket tables)
//! and new secrets.

use async_trait::async_trait;
use zendiloos_apps::application::ports::{AppHostError, AppShares, Secrets, ShareChoice};
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

    #[test]
    fn secrets_are_32_letters_and_digits() {
        let (one, two) = (RandomSecrets.generate(), RandomSecrets.generate());
        assert_eq!(one.len(), 32);
        assert!(one.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(one, two);
    }
}
