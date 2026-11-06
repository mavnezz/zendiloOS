//! Bounded context *Apps*: catalogs of apps (the community catalog built
//! in, others added by administrators) and, later, the apps installed from
//! them as Docker Compose projects under a policy (docs/architecture.md,
//! 4.9).

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
