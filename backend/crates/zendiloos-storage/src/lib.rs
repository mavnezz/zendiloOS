//! Bounded context *Storage*: the disks of the host, their SMART health,
//! software RAID arrays and the volumes (filesystems) on top of them.
//!
//! Read-only for now (M2); creating and changing arrays and volumes follows
//! in M3.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
