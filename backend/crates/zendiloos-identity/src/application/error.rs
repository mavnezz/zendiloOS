use std::time::Duration;

use zendiloos_kernel::PortError;

use crate::domain::{InvalidAccountName, PasswordRejected};

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error(transparent)]
    InvalidAccountName(#[from] InvalidAccountName),
    #[error(transparent)]
    PasswordRejected(#[from] PasswordRejected),
    /// Deliberately does not say whether the name or the password was wrong.
    #[error("wrong name or password")]
    InvalidCredentials,
    #[error("the current password is wrong")]
    WrongPassword,
    #[error("the account is disabled")]
    AccountDisabled,
    #[error("too many failed attempts")]
    TooManyAttempts { retry_after: Duration },
    #[error("the initial setup has already been completed")]
    SetupCompleted,
    #[error("wrong setup code")]
    InvalidSetupCode,
    #[error("not signed in")]
    Unauthenticated,
    #[error(transparent)]
    Port(#[from] PortError),
}
