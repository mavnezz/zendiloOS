//! Bounded context Backup: the App Center's plugin (rsync and the SSH
//! client from Debian), backup tasks from a share to another place on the
//! NAS (a share on another pool, a removable drive) or to an rsync server,
//! and from an rsync server into the NAS; their runs on a schedule, by
//! hand or when a drive is plugged in, as mirrors or dated versions; their
//! log, and restores (docs/architecture.md, 4.14).

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
