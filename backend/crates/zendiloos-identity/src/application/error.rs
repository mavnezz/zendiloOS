use std::time::Duration;

use zendiloos_kernel::PortError;

use crate::domain::text::InvalidText;
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
    InvalidText(#[from] InvalidText),
    #[error(transparent)]
    InvalidEmail(#[from] crate::domain::text::InvalidEmail),
    #[error("the account may not do this")]
    Forbidden,
    #[error("there is no such account")]
    AccountNotFound,
    #[error("there is no such group")]
    GroupNotFound,
    /// By another account or group, or by the operating system.
    #[error("the name is already taken")]
    NameTaken,
    #[error("at least one enabled administrator must remain")]
    LastAdmin,
    #[error("use your own settings to change your own account")]
    OwnAccount,
    #[error("an administrator sets this account's password")]
    PasswordLocked,
    #[error(transparent)]
    InvalidPolicy(#[from] crate::domain::InvalidPolicy),
    #[error(transparent)]
    InvalidSecurity(#[from] crate::domain::InvalidSecuritySettings),
    #[error(transparent)]
    InvalidAddressRange(#[from] crate::domain::InvalidAddressRange),
    /// Requests from the caller's address are refused.
    #[error("this address is blocked")]
    AddressBlocked,
    /// The block would lock the caller out.
    #[error("the address you are using would be blocked")]
    OwnAddress,
    /// Allowed addresses are never blocked.
    #[error("the address is allowed and is never blocked")]
    AddressAllowed,
    #[error("the address is not blocked")]
    BlockNotFound,
    #[error("there is no such session")]
    SessionNotFound,
    #[error(transparent)]
    Port(#[from] PortError),
}
