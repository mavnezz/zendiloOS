//! Bounded context Certificates: the ACME client plugin (acme.sh) with its
//! accounts, challenges and certificates, renewed by themselves, and the
//! web interface's ports and the certificate it shows.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
