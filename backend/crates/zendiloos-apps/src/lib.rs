//! Bounded context *Apps*: catalogs of apps (the community catalog built
//! in, others added by administrators), their apps filled in as templates
//! of administrators' Docker Compose projects, and the apps installed the
//! earlier way, which are only run and removed (docs/architecture.md, 4.9).

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
