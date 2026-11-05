use std::sync::Arc;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash as Phc;
use argon2::password_hash::{Error as HashError, PasswordHasher as _, PasswordVerifier as _};
use async_trait::async_trait;
use zendiloos_identity::application::ports::PasswordHasher;
use zendiloos_identity::domain::PasswordHash;
use zendiloos_kernel::{PortError, Secret};

use crate::blocking;

/// Argon2id with the library's default parameters (19 MiB, 2 passes, one
/// lane), as recommended by OWASP. Hashing runs on the blocking pool.
pub struct Argon2PasswordHasher {
    /// Verified against when an account does not exist, so that case costs
    /// as much time as a real check.
    dummy: Arc<PasswordHash>,
    /// At most this many run at once; the rest wait. Hundreds of requests at
    /// once would otherwise take every CPU and gigabytes of memory.
    running: Arc<tokio::sync::Semaphore>,
}

/// Argon2 runs at the same time: 19 MiB each.
pub const CONCURRENT_HASHES: usize = 2;

impl Argon2PasswordHasher {
    /// Computes the dummy hash, which takes a few dozen milliseconds.
    pub fn new() -> Result<Self, PortError> {
        Ok(Self {
            dummy: Arc::new(hash(b"zendiloos dummy password")?),
            running: Arc::new(tokio::sync::Semaphore::new(CONCURRENT_HASHES)),
        })
    }

    async fn permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, PortError> {
        self.running.acquire().await.map_err(|error| PortError::new("password hashing", error))
    }
}

fn hash(password: &[u8]) -> Result<PasswordHash, PortError> {
    Argon2::default()
        .hash_password(password)
        .map(|phc| PasswordHash::new(phc.to_string()))
        .map_err(|error| PortError::new("password hashing", error))
}

fn verify(password: &[u8], hash: &PasswordHash) -> Result<bool, PortError> {
    let phc = Phc::new(hash.as_str()).map_err(|error| PortError::new("stored password hash", error))?;
    match Argon2::default().verify_password(password, &phc) {
        Ok(()) => Ok(true),
        Err(HashError::PasswordInvalid) => Ok(false),
        Err(error) => Err(PortError::new("password verification", error)),
    }
}

#[async_trait]
impl PasswordHasher for Argon2PasswordHasher {
    async fn hash(&self, password: &Secret) -> Result<PasswordHash, PortError> {
        let password = password.clone();
        let _permit = self.permit().await?;
        blocking::run("password hashing", move || hash(password.expose().as_bytes())).await
    }

    async fn verify(&self, password: &Secret, stored: Option<&PasswordHash>) -> Result<bool, PortError> {
        let password = password.clone();
        let (hash, real) = match stored {
            Some(hash) => (Arc::new(hash.clone()), true),
            None => (Arc::clone(&self.dummy), false),
        };
        let _permit = self.permit().await?;
        let matches =
            blocking::run("password verification", move || verify(password.expose().as_bytes(), &hash))
                .await?;
        Ok(real && matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_are_salted_argon2id_and_verify() {
        let hasher = Argon2PasswordHasher::new().unwrap();
        let password = Secret::new("correct horse battery");

        let first = hasher.hash(&password).await.unwrap();
        let second = hasher.hash(&password).await.unwrap();

        assert!(first.as_str().starts_with("$argon2id$"), "{}", first.as_str());
        assert_ne!(first, second, "every hash has its own salt");
        assert!(hasher.verify(&password, Some(&first)).await.unwrap());
        assert!(!hasher.verify(&Secret::new("wrong"), Some(&first)).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn only_a_few_run_at_once() {
        let hasher = Arc::new(Argon2PasswordHasher::new().unwrap());
        let tasks: Vec<_> = (0..6)
            .map(|_| {
                let hasher = Arc::clone(&hasher);
                tokio::spawn(async move { hasher.hash(&Secret::new("correct horse battery")).await })
            })
            .collect();
        tokio::task::yield_now().await;
        assert!(hasher.running.available_permits() <= CONCURRENT_HASHES);
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(hasher.running.available_permits(), CONCURRENT_HASHES, "all permits back");
    }

    #[tokio::test]
    async fn a_missing_account_never_verifies() {
        let hasher = Argon2PasswordHasher::new().unwrap();
        assert!(!hasher.verify(&Secret::new("zendiloos dummy password"), None).await.unwrap());
    }

    #[tokio::test]
    async fn a_corrupt_stored_hash_is_an_error_not_a_mismatch() {
        let hasher = Argon2PasswordHasher::new().unwrap();
        let result = hasher.verify(&Secret::new("x"), Some(&PasswordHash::new("not a hash"))).await;
        assert!(result.is_err());
    }
}
