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
    #[error("the password is one of the most commonly used ones")]
    Common,
    #[error("the password must contain upper and lower case letters")]
    NeedsMixedCase,
    #[error("the password must contain a digit")]
    NeedsDigit,
    #[error("the password must contain a character that is neither a letter nor a digit")]
    NeedsSymbol,
}

/// Rules a new password must follow, set by an administrator.
///
/// Length counts most and is always required, as is not being a common
/// password or containing the account name; what a password is made of
/// and how long it lasts can be required on top, as some organisations
/// must. Both are off by default: they make passwords harder to remember,
/// not harder to guess (NIST SP 800-63B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordPolicy {
    pub min_length: u8,
    pub mixed_case: bool,
    pub digit: bool,
    pub symbol: bool,
    /// After this many days a password must be changed at the next login.
    pub max_age_days: Option<u16>,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_length: MIN_PASSWORD_CHARS as u8,
            mixed_case: false,
            digit: false,
            symbol: false,
            max_age_days: None,
        }
    }
}

/// The longest minimum length that can be required.
pub const MAX_MIN_LENGTH: u8 = 64;
/// Bounds of the maximum age: a month to two years.
pub const MAX_AGE_DAYS: std::ops::RangeInclusive<u16> = 30..=730;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPolicy {
    #[error("the minimum length is from {MIN_PASSWORD_CHARS} to {MAX_MIN_LENGTH} characters")]
    MinLength,
    #[error("a password lasts from 30 to 730 days")]
    MaxAge,
}

impl PasswordPolicy {
    pub fn checked(self) -> Result<Self, InvalidPolicy> {
        if !(MIN_PASSWORD_CHARS as u8..=MAX_MIN_LENGTH).contains(&self.min_length) {
            return Err(InvalidPolicy::MinLength);
        }
        if self.max_age_days.is_some_and(|days| !MAX_AGE_DAYS.contains(&days)) {
            return Err(InvalidPolicy::MaxAge);
        }
        Ok(self)
    }
}

/// The most used passwords long enough to pass the length rule.
const COMMON: &str = include_str!("common-passwords.txt");

fn is_common(lowered: &str) -> bool {
    COMMON.lines().filter(|line| !line.starts_with('#')).any(|line| line == lowered)
}

/// A password someone wants to set, accepted by the password policy.
#[derive(Debug)]
pub struct NewPassword(Secret);

impl NewPassword {
    pub fn parse(
        password: Secret,
        account: &AccountName,
        policy: &PasswordPolicy,
    ) -> Result<Self, PasswordRejected> {
        let value = password.expose();
        let min = usize::from(policy.min_length).max(MIN_PASSWORD_CHARS);
        if value.chars().count() < min {
            return Err(PasswordRejected::TooShort { min });
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
        if is_common(&lowered) {
            return Err(PasswordRejected::Common);
        }
        if policy.mixed_case
            && !(value.chars().any(char::is_uppercase) && value.chars().any(char::is_lowercase))
        {
            return Err(PasswordRejected::NeedsMixedCase);
        }
        if policy.digit && !value.chars().any(|c| c.is_numeric()) {
            return Err(PasswordRejected::NeedsDigit);
        }
        if policy.symbol && !value.chars().any(|c| !c.is_alphanumeric()) {
            return Err(PasswordRejected::NeedsSymbol);
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
        with(password, account, &PasswordPolicy::default())
    }

    fn with(password: &str, account: &str, policy: &PasswordPolicy) -> Result<(), PasswordRejected> {
        NewPassword::parse(Secret::new(password), &AccountName::parse(account).unwrap(), policy).map(|_| ())
    }

    #[test]
    fn rejects_common_passwords_whatever_their_case() {
        assert_eq!(check("qwertyuiop", "alice"), Err(PasswordRejected::Common));
        assert_eq!(check("Password12", "alice"), Err(PasswordRejected::Common));
        assert_eq!(check("correct horse battery", "alice"), Ok(()));
    }

    #[test]
    fn follows_the_rules_an_administrator_added() {
        let strict = PasswordPolicy {
            min_length: 14,
            mixed_case: true,
            digit: true,
            symbol: true,
            max_age_days: None,
        };
        assert_eq!(with("short enough", "alice", &strict), Err(PasswordRejected::TooShort { min: 14 }));
        assert_eq!(with("all lower case words", "alice", &strict), Err(PasswordRejected::NeedsMixedCase));
        assert_eq!(with("Mixed Case Words Only", "alice", &strict), Err(PasswordRejected::NeedsDigit));
        assert_eq!(with("Mixed Case and 4 Words", "alice", &strict), Ok(()), "a space is a symbol");
        assert_eq!(with("MixedCaseAnd4Words", "alice", &strict), Err(PasswordRejected::NeedsSymbol));
    }

    #[test]
    fn a_policy_cannot_ask_for_less_than_ten_characters() {
        let policy = |min_length, max_age_days| PasswordPolicy {
            min_length,
            max_age_days,
            ..PasswordPolicy::default()
        };
        assert_eq!(policy(9, None).checked(), Err(InvalidPolicy::MinLength));
        assert_eq!(policy(65, None).checked(), Err(InvalidPolicy::MinLength));
        assert_eq!(policy(12, Some(7)).checked(), Err(InvalidPolicy::MaxAge));
        assert!(policy(12, Some(90)).checked().is_ok());
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
