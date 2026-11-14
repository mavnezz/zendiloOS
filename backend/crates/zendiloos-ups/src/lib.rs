//! Bounded context UPS: an uninterruptible power supply through Network
//! UPS Tools (NUT) from Debian. Where the UPS is (on USB, on the network
//! over SNMP, or at another device that serves it), when the NAS shuts
//! down while it runs on battery, whether other devices may follow it,
//! and what its state tells (docs/architecture.md, 4.16).
//!
//! NUT does the watching and the shutting down, also while zendiloosd is
//! not running; zendiloOS keeps the settings, has the agent write NUT's
//! files from them, reads the UPS's state for its page and turns its
//! changes into notifications.

pub mod application;
pub mod domain;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
