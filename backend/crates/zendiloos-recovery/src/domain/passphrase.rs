//! The passphrase a backup is encrypted with. The backup holds the
//! database's password hashes, the certificates' keys, Samba's passwords
//! and the secrets kept for mail, backups and DNS: nobody without it can
//! open it, and nobody can restore it without it either.

use zendiloos_kernel::Secret;

/// At least this many characters.
pub const PASSPHRASE_MIN: usize = 12;
/// At most this many bytes.
pub const PASSPHRASE_MAX: usize = 1024;

#[derive(Debug, Clone)]
pub struct Passphrase(Secret);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPassphrase {
    #[error("the passphrase needs at least {PASSPHRASE_MIN} characters")]
    TooShort,
    #[error("the passphrase is longer than {PASSPHRASE_MAX} bytes")]
    TooLong,
    #[error("the passphrase may not hold control characters")]
    Control,
}

impl Passphrase {
    pub fn new(value: &str) -> Result<Self, InvalidPassphrase> {
        if value.chars().count() < PASSPHRASE_MIN {
            return Err(InvalidPassphrase::TooShort);
        }
        if value.len() > PASSPHRASE_MAX {
            return Err(InvalidPassphrase::TooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(InvalidPassphrase::Control);
        }
        Ok(Self(Secret::new(value)))
    }

    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passphrase_is_long_enough_and_one_line() {
        assert!(Passphrase::new("correct horse battery").is_ok());
        assert!(Passphrase::new("äöüäöüäöüäöü").is_ok(), "characters, not bytes");
        assert_eq!(Passphrase::new("short").unwrap_err(), InvalidPassphrase::TooShort);
        assert_eq!(Passphrase::new(&"x".repeat(1025)).unwrap_err(), InvalidPassphrase::TooLong);
        assert_eq!(Passphrase::new("twelve chars\nmore").unwrap_err(), InvalidPassphrase::Control);
        assert!(!format!("{:?}", Passphrase::new("correct horse battery").unwrap()).contains("horse"));
    }
}
