//! Copies of the database taken before migrations change it.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;

/// How many backups to keep; older ones are deleted after a new one is made.
pub(crate) const KEEP: usize = 5;

const PREFIX: &str = "zendiloos-";
const SUFFIX: &str = ".db";

/// Writes a consistent copy of the database into `dir` and prunes old
/// copies. Returns the path of the new backup.
///
/// The file is named `zendiloos-<UTC time>-schema-<version>.db`, so the
/// names sort chronologically and say which release can read them.
pub(crate) async fn create(
    pool: &SqlitePool,
    dir: &Path,
    schema: i64,
    now: SystemTime,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let path = reserve_file(dir, &format!("{PREFIX}{}-schema-{schema}", utc_compact(now)))?;

    // The database holds password hashes, so the copy must not be readable
    // by others. VACUUM INTO accepts an existing file only if it is empty,
    // which lets us create it with the right mode first.
    let target = path.to_str().ok_or("backup path is not valid UTF-8")?;
    if let Err(error) = sqlx::query("VACUUM INTO ?").bind(target).execute(pool).await {
        let _ = fs::remove_file(&path);
        return Err(error.into());
    }

    prune(dir, KEEP)?;
    Ok(path)
}

/// Creates an empty, private file `<stem>.db`, or `<stem>-2.db`, … if a
/// backup with that name already exists (two starts within one second).
fn reserve_file(dir: &Path, stem: &str) -> io::Result<PathBuf> {
    for attempt in 1..=100 {
        let name = if attempt == 1 { format!("{stem}{SUFFIX}") } else { format!("{stem}-{attempt}{SUFFIX}") };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("too many backups named {stem}")))
}

/// Deletes all but the `keep` newest backups in `dir`. Other files are left
/// alone.
pub(crate) fn prune(dir: &Path, keep: usize) -> io::Result<()> {
    let mut backups: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                name.starts_with(PREFIX) && name.ends_with(SUFFIX) && name.contains("-schema-")
            })
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(keep);
    for path in &backups[..excess] {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// `20261105T081953Z`: sortable and valid in file names.
fn utc_compact(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let secs = i64::try_from(secs).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let seconds_of_day = secs.rem_euclid(86_400);
    let (hour, minute, second) = (seconds_of_day / 3600, seconds_of_day / 60 % 60, seconds_of_day % 60);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// Gregorian date for a number of days since 1970-01-01.
/// Howard Hinnant's algorithm, <https://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn formats_utc_timestamps() {
        // Reference values computed independently with Python's datetime.
        assert_eq!(utc_compact(at(0)), "19700101T000000Z");
        assert_eq!(utc_compact(at(951_868_800)), "20000301T000000Z");
        assert_eq!(utc_compact(at(1_000_000_000)), "20010909T014640Z");
        assert_eq!(utc_compact(at(1_709_164_800)), "20240229T000000Z");
        assert_eq!(utc_compact(at(1_793_780_393)), "20261104T081953Z");
        assert_eq!(utc_compact(at(4_102_444_799)), "20991231T235959Z");
    }

    #[test]
    fn reserves_a_new_name_when_one_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let first = reserve_file(dir.path(), "zendiloos-x-schema-1").unwrap();
        let second = reserve_file(dir.path(), "zendiloos-x-schema-1").unwrap();
        assert!(first.ends_with("zendiloos-x-schema-1.db"));
        assert!(second.ends_with("zendiloos-x-schema-1-2.db"));
    }

    #[test]
    fn prune_keeps_the_newest_backups_and_ignores_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let names = [
            "zendiloos-20260101T000000Z-schema-1.db",
            "zendiloos-20260201T000000Z-schema-1.db",
            "zendiloos-20260301T000000Z-schema-2.db",
            "zendiloos-20260401T000000Z-schema-3.db",
            "unrelated.db",
            "zendiloos.db",
        ];
        for name in names {
            fs::write(dir.path().join(name), b"").unwrap();
        }

        prune(dir.path(), 2).unwrap();

        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "unrelated.db",
                "zendiloos-20260301T000000Z-schema-2.db",
                "zendiloos-20260401T000000Z-schema-3.db",
                "zendiloos.db",
            ]
        );
    }
}
