//! Where each kind of notification goes: kept in zendiloOS's notification
//! center, sent on which channels, or both.
//!
//! A kind is a notification's code, except that a data check that found
//! errors (`scrub_errors`) is a kind of its own: it matters where a routine
//! check does not. The administrator ticks, per kind, "in zendiloOS" and
//! each channel. A kind nobody ticked anything for yet follows defaults:
//! kept in zendiloOS, and sent on a channel if the channel's level (the
//! choice of the time before routes, kept as its baseline) carries the
//! most serious level the kind is raised at; so a channel for errors only
//! still gets a failing pool, and kinds added in a later version reach the
//! channels that would have carried them.

use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::PortError;

use crate::Level;

/// What a kind of notification is about, for grouping them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Pools, disks, data checks, removable drives.
    Storage,
    /// Folders on the volumes: snapshots.
    Folders,
    /// Updates of Debian and zendiloOS.
    System,
    /// Certificates the ACME client renews.
    Certificates,
    /// The firewall.
    Security,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Folders => "folders",
            Self::System => "system",
            Self::Certificates => "certificates",
            Self::Security => "security",
        }
    }
}

/// A kind of notification, as the routing table lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    pub key: &'static str,
    pub group: Group,
    /// The most serious level it is raised at: what decides the default
    /// for a channel, so a default never keeps an error from a channel
    /// that took errors.
    pub level: Level,
}

const fn kind(key: &'static str, group: Group, level: Level) -> Kind {
    Kind { key, group, level }
}

/// Every kind zendiloOS raises, in the order the table shows them.
pub const KINDS: [Kind; 15] = [
    // Degraded a warning; faulted, removed or unavailable an error.
    kind("pool_trouble", Group::Storage, Level::Error),
    kind("pool_recovered", Group::Storage, Level::Success),
    // SMART getting worse a warning, failing an error.
    kind("disk_health", Group::Storage, Level::Error),
    // 80 % a warning, 90 % an error.
    kind("volume_filling", Group::Storage, Level::Error),
    kind("scrub_errors", Group::Storage, Level::Warning),
    kind("scrub_finished", Group::Storage, Level::Info),
    // Without errors good news, with errors a warning.
    kind("resilver_finished", Group::Storage, Level::Warning),
    kind("removable_pulled", Group::Storage, Level::Warning),
    kind("snapshot_failed", Group::Folders, Level::Warning),
    kind("updates_available", Group::System, Level::Info),
    // Done a success; the ZFS module missing for the new kernel an error.
    kind("updates_installed", Group::System, Level::Error),
    kind("updates_failed", Group::System, Level::Error),
    kind("certificate_renewed", Group::Certificates, Level::Success),
    kind("certificate_renewal_failed", Group::Certificates, Level::Error),
    kind("firewall_blocked", Group::Security, Level::Warning),
];

/// The kind a notification of `code` at `level` belongs to.
pub fn kind_of(code: &str, level: Level) -> &str {
    match code {
        "scrub_finished" if matches!(level, Level::Warning | Level::Error) => "scrub_errors",
        other => other,
    }
}

/// The kind of that key, if zendiloOS knows it.
pub fn find(key: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.key == key)
}

/// Whether a channel of the level `baseline` carries a notification of
/// `level`: info carries everything, warning also the good news that ends
/// a warning, error only errors.
pub fn carries(baseline: Level, level: Level) -> bool {
    match baseline {
        Level::Info => true,
        Level::Success | Level::Warning => level != Level::Info,
        Level::Error => level == Level::Error,
    }
}

/// The choices made: per kind, whether it is kept in zendiloOS, and per
/// channel and kind, whether it goes out there. What is not here follows
/// the defaults (see the module).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Routes {
    pub in_app: BTreeMap<String, bool>,
    /// By channel ID and kind.
    pub channels: BTreeMap<(i64, String), bool>,
}

impl Routes {
    /// Whether a notification of `kind` is kept in the notification center.
    pub fn in_app(&self, kind: &str) -> bool {
        self.in_app.get(kind).copied().unwrap_or(true)
    }

    /// Whether a notification of `kind` at `level` goes out on the channel
    /// `channel` of the level `baseline`.
    pub fn to_channel(&self, channel: i64, baseline: Level, kind: &str, level: Level) -> bool {
        match self.channels.get(&(channel, kind.to_owned())) {
            Some(&chosen) => chosen,
            None => carries(baseline, find(kind).map_or(level, |kind| kind.level)),
        }
    }
}

#[async_trait]
pub trait RouteRepository: Send + Sync {
    async fn routes(&self) -> Result<Routes, PortError>;

    /// Replaces every choice with `routes`.
    async fn replace(&self, routes: &Routes) -> Result<(), PortError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_check_that_found_errors_is_a_kind_of_its_own() {
        assert_eq!(kind_of("scrub_finished", Level::Info), "scrub_finished");
        assert_eq!(kind_of("scrub_finished", Level::Warning), "scrub_errors");
        assert_eq!(kind_of("pool_trouble", Level::Error), "pool_trouble");
        assert_eq!(kind_of("something_new", Level::Info), "something_new");
        assert!(KINDS.iter().all(|kind| find(kind.key) == Some(kind)));
        assert_eq!(find("something_new"), None);
        assert_eq!(
            [Group::Storage, Group::Folders, Group::System, Group::Certificates, Group::Security]
                .map(Group::as_str),
            ["storage", "folders", "system", "certificates", "security"]
        );
    }

    #[test]
    fn what_was_not_chosen_follows_the_channels_level() {
        let mut routes = Routes::default();
        assert!(routes.in_app("pool_trouble"), "kept by default");
        assert!(routes.to_channel(1, Level::Warning, "pool_trouble", Level::Warning));
        assert!(routes.to_channel(1, Level::Warning, "pool_recovered", Level::Success), "the good news");
        assert!(!routes.to_channel(1, Level::Warning, "scrub_finished", Level::Info));
        assert!(routes.to_channel(1, Level::Warning, "scrub_errors", Level::Warning));
        // By the kind's most serious level: a channel for errors only gets
        // all of a kind that can be an error, and none of one that cannot.
        assert!(routes.to_channel(1, Level::Error, "pool_trouble", Level::Error));
        assert!(routes.to_channel(1, Level::Error, "pool_trouble", Level::Warning));
        assert!(!routes.to_channel(1, Level::Error, "scrub_errors", Level::Warning));
        assert!(!routes.to_channel(1, Level::Error, "pool_recovered", Level::Success));
        assert!(routes.to_channel(1, Level::Info, "scrub_finished", Level::Info));
        // A kind this version does not know goes by the notification's level.
        assert!(routes.to_channel(1, Level::Error, "something_new", Level::Error));
        assert!(!routes.to_channel(1, Level::Error, "something_new", Level::Warning));

        routes.in_app.insert("scrub_finished".into(), false);
        routes.channels.insert((1, "pool_trouble".into()), false);
        routes.channels.insert((1, "scrub_finished".into()), true);
        assert!(!routes.in_app("scrub_finished"));
        assert!(!routes.to_channel(1, Level::Warning, "pool_trouble", Level::Error), "chosen off");
        assert!(routes.to_channel(1, Level::Warning, "scrub_finished", Level::Info), "chosen on");
        assert!(routes.to_channel(2, Level::Warning, "pool_trouble", Level::Warning), "another channel");
    }

    #[test]
    fn levels_carry_as_before() {
        assert!(carries(Level::Info, Level::Info));
        assert!(!carries(Level::Warning, Level::Info) && carries(Level::Warning, Level::Success));
        assert!(carries(Level::Success, Level::Error));
        assert!(!carries(Level::Error, Level::Warning) && carries(Level::Error, Level::Error));
    }
}
