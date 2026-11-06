//! Updates (docs/architecture.md, 4.10): Debian's packages through apt and
//! zendiloOS's own releases from GitHub, in the order OPNsense keeps:
//! whatever needs no restart first, the kernel last, a restart only when
//! asked.

pub mod application;
pub mod domain;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
