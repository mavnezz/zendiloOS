//! zendiloOS's network settings as last confirmed, which its database
//! keeps, against its systemd-networkd files as they are: what differs, and
//! the change that writes the settings back.
//!
//! The settings are kept as the files said once a change was confirmed,
//! and both sides are read by [`Configuration::read`], so only what
//! zendiloOS's settings say counts (addresses, DHCP, name servers, ports,
//! VLANs, a device's kind, members, mode, STP and MAC address), not how a
//! file is written: a newer zendiloOS writing the same settings otherwise
//! differs in nothing.

use super::devices::{Configuration, Device, Edit, LinkFile, Subject};

/// What a difference is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// A bridge, bond or VLAN zendiloOS made: its `.netdev`.
    Device,
    /// An interface's settings: its `.network`.
    Interface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifferenceKind {
    /// zendiloOS's settings have it, the files not: lost.
    Missing,
    /// The file says otherwise than zendiloOS's settings.
    Changed,
    /// A file of zendiloOS's its settings do not have: written outside it.
    Added,
}

/// One way in which the files differ from zendiloOS's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub part: Part,
    /// The device's or interface's name.
    pub name: String,
    pub kind: DifferenceKind,
}

/// Where the files (`now`) differ from the settings `kept`, devices first,
/// each by name.
pub fn differences(kept: &Configuration, now: &Configuration) -> Vec<Difference> {
    let mut found = Vec::new();
    compare(&kept.devices, &now.devices, |device: &Device| device.name.as_str(), Part::Device, &mut found);
    compare(
        &kept.links,
        &now.links,
        |link: &LinkFile| link.settings.interface.as_str(),
        Part::Interface,
        &mut found,
    );
    found
}

fn compare<T: PartialEq>(
    kept: &[T],
    now: &[T],
    name: impl Fn(&T) -> &str,
    part: Part,
    found: &mut Vec<Difference>,
) {
    let mut names: Vec<&str> = kept.iter().chain(now).map(&name).collect();
    names.sort_unstable();
    names.dedup();
    for which in names {
        let had = kept.iter().find(|item| name(item) == which);
        let has = now.iter().find(|item| name(item) == which);
        let kind = match (had, has) {
            (Some(had), Some(has)) if had == has => continue,
            (Some(_), Some(_)) => DifferenceKind::Changed,
            (Some(_), None) => DifferenceKind::Missing,
            (None, _) => DifferenceKind::Added,
        };
        found.push(Difference { part, name: which.to_owned(), kind });
    }
}

/// The change that makes the files say the settings `kept` again: each
/// device and interface file missing or changed written as kept, a device
/// zendiloOS's settings do not have removed (its own `.network` with it),
/// an interface's file they do not have taken away. None where nothing
/// differs.
pub fn restoring(kept: &Configuration, now: &Configuration) -> Option<Edit> {
    let found = differences(kept, now);
    if found.is_empty() {
        return None;
    }
    let of = |part: Part, kind: DifferenceKind| {
        found
            .iter()
            .filter(move |difference| difference.part == part && difference.kind == kind)
            .map(|difference| difference.name.clone())
    };
    let removed: Vec<String> = of(Part::Device, DifferenceKind::Added).collect();
    let written = |part| {
        let mut names: Vec<String> = of(part, DifferenceKind::Missing).collect();
        names.extend(of(part, DifferenceKind::Changed));
        names.sort_unstable();
        names
    };
    let devices: Vec<Device> = written(Part::Device)
        .iter()
        .filter_map(|name| kept.devices.iter().find(|device| device.name == *name).cloned())
        .collect();
    let links: Vec<LinkFile> = written(Part::Interface)
        .iter()
        .filter_map(|name| kept.links.iter().find(|link| link.settings.interface == *name).cloned())
        .collect();
    let unset: Vec<String> =
        of(Part::Interface, DifferenceKind::Added).filter(|interface| !removed.contains(interface)).collect();
    Some(Edit { subject: Subject::Restore, devices, removed, links, unset, effects: Vec::new() })
}

#[cfg(test)]
mod tests;
