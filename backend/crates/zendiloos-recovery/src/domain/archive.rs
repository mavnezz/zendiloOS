//! A configuration backup: one file, `zendiloos-config-<name>-<UTC
//! time>.tar.zst.age`, a tar archive compressed with zstd and encrypted
//! with age's passphrase (scrypt); inside, a manifest first, then the
//! database and the host's files. The names sort as the times do, so the
//! newest are kept by name alone.

use zendiloos_kernel::Timestamp;

/// The archive's layout; a backup of another is refused.
pub const FORMAT: u32 = 1;
pub const PREFIX: &str = "zendiloos-config-";
pub const SUFFIX: &str = ".tar.zst.age";

/// What a backup says of itself, before anything of it is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    /// zendiloOS's version that made it.
    pub version: String,
    /// The database's schema: its newest migration.
    pub schema: i64,
    pub host_name: String,
    pub created_at: Timestamp,
}

/// The file name of a backup of `host` made at `at`: its name as a file
/// name takes it, and the time as `20261112T030000Z`.
pub fn archive_name(host: &str, at: Timestamp) -> String {
    let host: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '-' })
        .take(63)
        .collect();
    let host = if host.is_empty() { "nas".to_owned() } else { host };
    let t = at.utc();
    format!(
        "{PREFIX}{host}-{:04}{:02}{:02}T{:02}{:02}{:02}Z{SUFFIX}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// The time part of a backup's name (`20261112T030000Z`), if it is one.
pub fn stamp(name: &str) -> Option<&str> {
    let middle = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let stamp = middle.rsplit('-').next()?;
    let digits = stamp.len() == 16
        && stamp.as_bytes()[8] == b'T'
        && stamp.ends_with('Z')
        && stamp.bytes().enumerate().all(|(at, b)| at == 8 || at == 15 || b.is_ascii_digit());
    (digits && middle.len() > stamp.len() + 1).then_some(stamp)
}

/// Of the files in a target folder, the backups to remove so that the
/// newest `keep` stay; other files are never among them.
pub fn to_remove(names: &[String], keep: usize) -> Vec<String> {
    let mut backups: Vec<(&str, &String)> =
        names.iter().filter_map(|name| stamp(name).map(|stamp| (stamp, name))).collect();
    backups.sort();
    let excess = backups.len().saturating_sub(keep.max(1));
    backups[..excess].iter().map(|(_, name)| (*name).clone()).collect()
}

/// How a backup's database stands to this release's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFit {
    /// The same migrations.
    Current,
    /// Older: the migrations it lacks run when it is opened.
    Older,
    /// Newer than this release knows: refused, never guessed at.
    Newer,
}

pub fn fit(found: i64, supported: i64) -> SchemaFit {
    match found.cmp(&supported) {
        std::cmp::Ordering::Equal => SchemaFit::Current,
        std::cmp::Ordering::Less => SchemaFit::Older,
        std::cmp::Ordering::Greater => SchemaFit::Newer,
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::UtcTime;

    use super::*;

    fn at(day: u8, hour: u8) -> Timestamp {
        UtcTime { year: 2026, month: 11, day, hour, minute: 0, second: 0 }.timestamp().unwrap()
    }

    #[test]
    fn names_say_the_machine_and_the_time() {
        assert_eq!(archive_name("office", at(12, 3)), "zendiloos-config-office-20261112T030000Z.tar.zst.age");
        assert_eq!(archive_name("My NAS", at(12, 3)), "zendiloos-config-my-nas-20261112T030000Z.tar.zst.age");
        assert_eq!(archive_name("", at(12, 3)), "zendiloos-config-nas-20261112T030000Z.tar.zst.age");
        assert_eq!(stamp(&archive_name("zendiloos-vm2", at(1, 0))), Some("20261101T000000Z"));
        for other in [
            "photo.jpg",
            "zendiloos-config-20261112T030000Z.tar.zst.age",
            "zendiloos-config-nas-2026111T030000Z.tar.zst.age",
            "zendiloos-config-nas-20261112T030000Z.tar.zst",
        ] {
            assert_eq!(stamp(other), None, "{other}");
        }
    }

    #[test]
    fn the_newest_are_kept_and_nothing_else_goes() {
        let names: Vec<String> = [
            archive_name("nas", at(3, 3)),
            "notes.txt".to_owned(),
            archive_name("nas", at(1, 3)),
            archive_name("old-name", at(2, 3)),
            archive_name("nas", at(4, 3)),
        ]
        .into();
        assert_eq!(to_remove(&names, 2), [archive_name("nas", at(1, 3)), archive_name("old-name", at(2, 3))]);
        assert!(to_remove(&names, 10).is_empty());
        assert_eq!(to_remove(&names, 0).len(), 3, "the newest always stays");
    }

    #[test]
    fn a_newer_schema_does_not_fit() {
        assert_eq!(fit(53, 55), SchemaFit::Older);
        assert_eq!(fit(55, 55), SchemaFit::Current);
        assert_eq!(fit(56, 55), SchemaFit::Newer);
    }
}
