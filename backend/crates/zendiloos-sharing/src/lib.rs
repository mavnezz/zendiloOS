//! Bounded context *Sharing*: shares on the volumes, offered over SMB and
//! NFS, and who may use them.
//!
//! A share is a ZFS dataset below a volume. Its permissions live in the
//! database and are applied twice (docs/architecture.md, 4.4): in the Samba
//! configuration and as POSIX ACLs on its directory, so that SMB, NFS and
//! the file manager see the same rules.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
