//! Personal folders: one per account that has one, in a dataset of their
//! own on one pool, offered over SMB as the share `home` and in the file
//! manager; only the account itself reaches it.

use super::ShareId;

/// The ID the file manager knows the personal folder by; shares count
/// from 1.
pub const HOME_SHARE: ShareId = ShareId::new(0);

/// Where the personal folders are: the pool and its volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomesLocation {
    pub pool: String,
    pub volume: u16,
}

/// An account with a personal folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub account: i64,
    /// How much it may store there; `None`: as much as the pool has.
    pub quota_bytes: Option<u64>,
}

/// The share `home` as the SMB server should offer it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomesSpec {
    pub volume: u16,
    /// The accounts with a personal folder, by name.
    pub users: Vec<String>,
}
