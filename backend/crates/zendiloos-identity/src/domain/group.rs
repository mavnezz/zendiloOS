use zendiloos_kernel::Timestamp;

use super::{AccountId, AccountName, InvalidAccountName};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(i64);

impl GroupId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Name of a group, which is also its Linux group name. Follows the rules
/// of [`AccountName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupName(AccountName);

impl GroupName {
    pub fn parse(name: &str) -> Result<Self, InvalidAccountName> {
        AccountName::parse(name).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for GroupName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A set of accounts, for granting access to shares together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: GroupId,
    pub name: GroupName,
    pub description: Option<String>,
    /// Sorted, without duplicates.
    pub members: Vec<AccountId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGroup {
    pub name: GroupName,
    pub description: Option<String>,
    pub members: Vec<AccountId>,
    pub created_at: Timestamp,
}
