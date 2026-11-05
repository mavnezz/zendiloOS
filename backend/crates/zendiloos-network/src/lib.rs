//! Bounded context *Network*: how the machine is connected (interfaces,
//! addresses, default routes, name servers) and, where systemd-networkd
//! manages it, changing an interface's settings with confirmation.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
