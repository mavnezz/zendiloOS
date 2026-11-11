//! Bounded context Virtual machines: the App Center's plugin (libvirt and
//! QEMU from Debian, the VMs' storage on a pool), VMs made from an OS
//! preset and run through libvirt, and their consoles (docs/architecture.md,
//! 4.13). A proof of concept.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
