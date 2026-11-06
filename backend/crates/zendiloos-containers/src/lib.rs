//! Bounded context *Containers*: the Docker engine as administrators see
//! it. Every container, image and network on it, whoever started them;
//! the Compose projects, among them those of administrators, who may run
//! any Compose file after confirming what it is allowed to do; how much the
//! containers use; whether newer images are published, and taking them on
//! a schedule; and moving Docker's data to another pool.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
