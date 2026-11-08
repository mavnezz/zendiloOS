//! Bounded context Firewall: one ordered list of rules saying who reaches
//! which service of the NAS and the ports its Docker containers publish,
//! what happens when no rule matches, and the services the rules block
//! from everywhere, told once.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
