use std::fmt;

use zendiloos_kernel::{Actor, Timestamp};

use super::{PasswordHash, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountId(i64);

impl AccountId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Login name of an account, which is also its Linux account name.
///
/// A lowercase letter followed by up to 31 lowercase letters, digits, `_` or
/// `-`. That is valid unchanged for `useradd`, Samba and file ACLs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAccountName {
    #[error("the name must not be empty")]
    Empty,
    #[error("the name must be at most {max} characters long")]
    TooLong { max: usize },
    #[error("the name must start with a lowercase letter")]
    InvalidStart,
    #[error("the name may only contain lowercase letters, digits, '_' and '-'")]
    InvalidCharacter,
    #[error("the name {0:?} is reserved for the system")]
    Reserved(String),
}

impl AccountName {
    pub const MAX_LEN: usize = 32;

    /// Accounts of a Debian base system. Accounts installed later by other
    /// packages are caught when the Linux account is created.
    const RESERVED: [&str; 17] = [
        "root", "daemon", "bin", "sys", "sync", "games", "man", "lp", "mail", "news", "uucp", "proxy",
        "www-data", "backup", "list", "irc", "nobody",
    ];

    pub fn parse(name: &str) -> Result<Self, InvalidAccountName> {
        let first = name.chars().next().ok_or(InvalidAccountName::Empty)?;
        if name.len() > Self::MAX_LEN {
            return Err(InvalidAccountName::TooLong { max: Self::MAX_LEN });
        }
        if !first.is_ascii_lowercase() {
            return Err(InvalidAccountName::InvalidStart);
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            return Err(InvalidAccountName::InvalidCharacter);
        }
        if Self::RESERVED.contains(&name) || name.starts_with("systemd-") || name.starts_with("zendiloos") {
            return Err(InvalidAccountName::Reserved(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A person's account: valid for the web interface, SMB and the file
/// manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub name: AccountName,
    pub full_name: Option<String>,
    /// What the account is for, as administrators note it.
    pub description: Option<String>,
    /// Where to reach its owner.
    pub email: Option<String>,
    pub role: Role,
    pub password_hash: PasswordHash,
    /// Set for accounts an administrator created with an initial password,
    /// and when the password is older than the policy allows.
    pub password_change_required: bool,
    /// Only administrators set its password; it never has to change it.
    pub password_locked: bool,
    pub password_changed_at: Timestamp,
    pub disabled: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Account {
    /// This account as the actor of an audit entry.
    pub fn actor(&self) -> Actor {
        Actor { account_id: self.id.get(), name: self.name.to_string() }
    }
}

/// An account that is about to be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub name: AccountName,
    pub full_name: Option<String>,
    pub description: Option<String>,
    pub email: Option<String>,
    pub role: Role,
    pub password_hash: PasswordHash,
    pub password_change_required: bool,
    pub password_locked: bool,
    pub created_at: Timestamp,
}

impl NewAccount {
    /// The account as stored with `id`.
    pub fn stored(self, id: AccountId) -> Account {
        Account {
            id,
            name: self.name,
            full_name: self.full_name,
            description: self.description,
            email: self.email,
            role: self.role,
            password_hash: self.password_hash,
            password_change_required: self.password_change_required,
            password_locked: self.password_locked,
            password_changed_at: self.created_at,
            disabled: false,
            created_at: self.created_at,
            updated_at: self.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_linux_style_names() {
        for name in ["alice", "a", "bob-smith", "carol_2", "d0", &"x".repeat(32)] {
            assert_eq!(AccountName::parse(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn rejects_malformed_names() {
        assert_eq!(AccountName::parse(""), Err(InvalidAccountName::Empty));
        assert_eq!(AccountName::parse(&"x".repeat(33)), Err(InvalidAccountName::TooLong { max: 32 }));
        for name in ["Alice", "1alice", "-alice", "_alice"] {
            assert_eq!(AccountName::parse(name), Err(InvalidAccountName::InvalidStart), "{name}");
        }
        for name in ["al ice", "alice!", "alicE", "al.ice", "al/ice", "alïce"] {
            assert_eq!(AccountName::parse(name), Err(InvalidAccountName::InvalidCharacter), "{name}");
        }
    }

    #[test]
    fn rejects_system_names() {
        for name in ["root", "www-data", "nobody", "systemd-network", "zendiloos", "zendiloos-agent"] {
            assert!(matches!(AccountName::parse(name), Err(InvalidAccountName::Reserved(_))), "{name}");
        }
    }
}
