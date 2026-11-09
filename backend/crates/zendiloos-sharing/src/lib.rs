//! Bounded context *Sharing*: shares on the volumes, offered over SMB and
//! NFS, and who may use them.
//!
//! A share is a ZFS dataset below a volume. Its rules live in the database
//! and hold for the share as a whole (docs/architecture.md, 4.4): Samba
//! enforces them in each share's section, this context for the file
//! manager; NFS clients are named by host. On disk every share has the one
//! ACL of the group `zendiloos-files`, whatever its rules.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
