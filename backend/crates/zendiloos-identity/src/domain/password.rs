use std::fmt;

use zendiloos_kernel::Secret;

use super::AccountName;

pub const MIN_PASSWORD_CHARS: usize = 10;
/// Bounds the work a single login can cause.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// A stored password hash (Argon2id, PHC string format). Opaque to the
/// domain; only the password hasher port understands it.
#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHash(String);

impl PasswordHash {
    pub fn new(phc: impl Into<String>) -> Self {
        Self(phc.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PasswordHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PasswordHash(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordRejected {
    #[error("the password must be at least {min} characters long")]
    TooShort { min: usize },
    #[error("the password must be at most {max} bytes long")]
    TooLong { max: usize },
    #[error("the password must not contain the account name")]
    ContainsAccountName,
    /// Line breaks and NUL cannot be passed on to Samba.
    #[error("the password must not contain line breaks or control characters")]
    ControlCharacter,
}

/// A password someone wants to set, accepted by the password policy.
///
/// The policy favours length over composition rules. **Not yet
/// implemented:** a check against a list of commonly used passwords.
#[derive(Debug)]
pub struct NewPassword(Secret);

impl NewPassword {
    pub fn parse(password: Secret, account: &AccountName) -> Result<Self, PasswordRejected> {
        let value = password.expose();
        if value.chars().count() < MIN_PASSWORD_CHARS {
            return Err(PasswordRejected::TooShort { min: MIN_PASSWORD_CHARS });
        }
        if value.len() > MAX_PASSWORD_BYTES {
            return Err(PasswordRejected::TooLong { max: MAX_PASSWORD_BYTES });
        }
        if value.chars().any(char::is_control) {
            return Err(PasswordRejected::ControlCharacter);
        }
        // Very short names would match too many ordinary passwords.
        let lowered = value.to_lowercase();
        let name = account.as_str();
        if lowered == name || (name.len() >= 4 && lowered.contains(name)) {
            return Err(PasswordRejected::ContainsAccountName);
        }
        Ok(Self(password))
    }

    pub fn secret(&self) -> &Secret {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(password: &str, account: &str) -> Result<(), PasswordRejected> {
        NewPassword::parse(Secret::new(password), &AccountName::parse(account).unwrap()).map(|_| ())
    }

    #[test]
    fn accepts_long_enough_passwords() {
        assert_eq!(check("correct horse battery", "alice"), Ok(()));
        assert_eq!(check("äöüäöüäöüä", "alice"), Ok(()), "length counts characters, not bytes");
    }

    #[test]
    fn rejects_short_and_overlong_passwords() {
        assert_eq!(check("123456789", "alice"), Err(PasswordRejected::TooShort { min: 10 }));
        assert_eq!(check(&"a".repeat(1025), "alice"), Err(PasswordRejected::TooLong { max: 1024 }));
    }

    #[test]
    fn rejects_control_characters() {
        assert_eq!(check("correct horse\nbattery", "alice"), Err(PasswordRejected::ControlCharacter));
        assert_eq!(check("correct horse\0battery", "alice"), Err(PasswordRejected::ControlCharacter));
    }

    #[test]
    fn rejects_passwords_containing_the_account_name() {
        assert_eq!(check("my-ALICE-password", "alice"), Err(PasswordRejected::ContainsAccountName));
        assert_eq!(
            check("bobsecret12", "bob"),
            Ok(()),
            "names shorter than four characters only match exactly"
        );
    }

    #[test]
    fn hash_debug_output_is_redacted() {
        assert_eq!(format!("{:?}", PasswordHash::new("$argon2id$v=19$...")), "PasswordHash(***)");
    }
}
