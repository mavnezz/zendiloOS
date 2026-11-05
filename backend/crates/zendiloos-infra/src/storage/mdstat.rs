//! `/proc/mdstat` into software RAID arrays.
//!
//! The format is the kernel's (drivers/md/md.c, `md_seq_show`):
//!
//! ```text
//! md0 : active raid1 sdc[2] sdb[0](F)
//!       8379392 blocks super 1.2 [2/1] [U_]
//!       [>....................]  recovery =  2.4% (201984/8379392) finish=0.6min speed=201984K/sec
//! ```

use zendiloos_storage::domain::{Array, ArrayMember, MemberState, Slots, SyncOperation, SyncProgress};

use super::ParseError;

pub fn parse(mdstat: &str) -> Result<Vec<Array>, ParseError> {
    let mut arrays: Vec<Array> = Vec::new();
    for line in mdstat.lines() {
        if let Some((name, rest)) = line.split_once(" : ").filter(|(name, _)| name.starts_with("md")) {
            arrays.push(header(name.trim(), rest)?);
        } else if line.starts_with(char::is_whitespace)
            && let Some(array) = arrays.last_mut()
        {
            detail(array, line.trim());
        }
    }
    Ok(arrays)
}

/// `active (auto-read-only) raid1 sdc[2] sdb[0](F)` or `inactive sdd[0](S)`.
fn header(name: &str, rest: &str) -> Result<Array, ParseError> {
    let invalid = |reason: &str| ParseError::new("mdstat", format!("{name}: {reason}"));
    let mut words = rest.split_whitespace().peekable();
    let active = match words.next() {
        Some("active") => true,
        Some("inactive") => false,
        _ => return Err(invalid("no state")),
    };
    let mut array = Array {
        name: name.to_owned(),
        level: None,
        active,
        read_only: false,
        size_bytes: None,
        slots: None,
        members: Vec::new(),
        sync: None,
    };
    for word in words {
        if word.starts_with('(') {
            array.read_only |= word.contains("read-only");
        } else if let Some((device, after)) = word.split_once('[') {
            let (index, flags) = after.split_once(']').ok_or_else(|| invalid("unterminated member"))?;
            let state = match flags {
                f if f.contains("(F)") => MemberState::Faulty,
                f if f.contains("(S)") => MemberState::Spare,
                f if f.contains("(R)") => MemberState::Replacement,
                f if f.contains("(J)") => MemberState::Journal,
                // (W), write-mostly, is a preference, not a state.
                _ => MemberState::Active,
            };
            array.members.push(ArrayMember {
                device: device.to_owned(),
                index: index.parse().map_err(|_| invalid("member index"))?,
                state,
            });
        } else if array.level.is_none() {
            array.level = Some(word.to_owned());
        }
    }
    array.members.sort_by_key(|member| member.index);
    Ok(array)
}

/// The indented lines below a header; unknown ones (bitmap, …) are skipped.
fn detail(array: &mut Array, line: &str) {
    if let Some((blocks, rest)) = line.split_once(" blocks") {
        // Sizes are in KiB.
        array.size_bytes = blocks.trim().parse::<u64>().ok().map(|kib| kib * 1024);
        array.slots = rest.split_whitespace().find_map(slots);
    } else if let Some(sync) = progress(line) {
        array.sync = Some(sync);
    }
}

/// `[2/1]`
fn slots(word: &str) -> Option<Slots> {
    let (total, working) = word.strip_prefix('[')?.strip_suffix(']')?.split_once('/')?;
    Some(Slots { total: total.parse().ok()?, working: working.parse().ok()? })
}

/// `[>....]  recovery =  2.4% (201984/8379392) finish=0.6min speed=201984K/sec`
/// or, while waiting for another array on the same disks, `resync=DELAYED`.
fn progress(line: &str) -> Option<SyncProgress> {
    // Without the progress bar.
    let line = match line.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(_, after)| after),
        None => line,
    };
    let (operation, rest) = line.split_once('=')?;
    let operation = match operation.trim() {
        "resync" => SyncOperation::Resync,
        "recovery" => SyncOperation::Recovery,
        "check" => SyncOperation::Check,
        "repair" => SyncOperation::Repair,
        "reshape" => SyncOperation::Reshape,
        _ => return None,
    };
    let rest = rest.trim();
    let percent = rest.split_once('%').and_then(|(percent, _)| percent.trim().parse().ok());
    let field =
        |key: &str| rest.split_whitespace().find_map(|word| word.strip_prefix(key)).map(str::to_owned);
    let remaining_seconds = field("finish=")
        .and_then(|value| value.strip_suffix("min")?.parse::<f64>().ok())
        .map(|minutes| (minutes * 60.0).round() as u64);
    let speed_bytes_per_second = field("speed=")
        .and_then(|value| value.strip_suffix("K/sec")?.parse::<u64>().ok())
        .map(|kib| kib * 1024);
    Some(SyncProgress { operation, percent, remaining_seconds, speed_bytes_per_second })
}

#[cfg(test)]
mod tests {
    use zendiloos_storage::domain::ArrayHealth;

    use super::*;

    fn only(mdstat: &str) -> Array {
        let mut arrays = parse(mdstat).unwrap();
        assert_eq!(arrays.len(), 1);
        arrays.remove(0)
    }

    #[test]
    fn a_clean_mirror() {
        let array = only(include_str!("fixtures/mdstat-raid1-clean.txt"));
        assert_eq!(array.name, "md0");
        assert_eq!(array.level.as_deref(), Some("raid1"));
        assert!(array.active && !array.read_only);
        assert_eq!(array.size_bytes, Some(8_379_392 * 1024));
        assert_eq!(array.slots, Some(Slots { total: 2, working: 2 }));
        let members: Vec<_> = array.members.iter().map(|m| (m.device.as_str(), m.index, m.state)).collect();
        assert_eq!(members, [("sdb", 0, MemberState::Active), ("sdc", 1, MemberState::Active)]);
        assert_eq!(array.sync, None);
        assert_eq!(array.health(), ArrayHealth::Healthy);
    }

    #[test]
    fn a_mirror_with_a_failed_disk() {
        let array = only(include_str!("fixtures/mdstat-raid1-degraded.txt"));
        assert_eq!(array.slots, Some(Slots { total: 2, working: 1 }));
        assert_eq!(array.members[1].state, MemberState::Faulty);
        assert_eq!(array.health(), ArrayHealth::Degraded);
    }

    #[test]
    fn a_mirror_rebuilding_onto_a_new_disk() {
        let array = only(include_str!("fixtures/mdstat-raid1-recovery.txt"));
        assert_eq!(
            array.sync,
            Some(SyncProgress {
                operation: SyncOperation::Recovery,
                percent: Some(2.4),
                remaining_seconds: Some(36),
                speed_bytes_per_second: Some(201_984 * 1024),
            })
        );
        assert_eq!(array.health(), ArrayHealth::Rebuilding);
    }

    #[test]
    fn a_new_mirror_resyncing() {
        let array = only(include_str!("fixtures/mdstat-raid1-resync.txt"));
        let sync = array.sync.as_ref().unwrap();
        assert_eq!((sync.operation, sync.percent), (SyncOperation::Resync, Some(9.5)));
        assert_eq!(array.health(), ArrayHealth::Healthy);
    }

    /// Written after the kernel's format, not captured: these states are
    /// hard to produce on purpose.
    #[test]
    fn inactive_read_only_striped_and_waiting_arrays() {
        let mdstat = "\
Personalities : [raid0] [raid1] [raid6] [raid5] [raid4]
md2 : inactive sdd[0](S)
      8379392 blocks super 1.2

md1 : active (auto-read-only) raid0 sdf[1] sde[0]
      16758784 blocks super 1.2 512k chunks

md3 : active raid5 sdg[0] sdh[1] sdi[3] sdj[4](S)
      16758784 blocks super 1.2 level 5, 512k chunk, algorithm 2 [3/3] [UUU]
      \tresync=DELAYED
      bitmap: 0/1 pages [0KB], 65536KB chunk

unused devices: <none>
";
        let arrays = parse(mdstat).unwrap();
        let [inactive, striped, waiting] = arrays.as_slice() else { panic!("{arrays:?}") };

        assert!(!inactive.active);
        assert_eq!(inactive.level, None);
        assert_eq!(inactive.members[0].state, MemberState::Spare);
        assert_eq!(inactive.health(), ArrayHealth::Inactive);

        assert!(striped.read_only);
        assert_eq!(striped.level.as_deref(), Some("raid0"));
        assert_eq!(striped.slots, None);
        assert_eq!(striped.size_bytes, Some(16_758_784 * 1024));

        assert_eq!(waiting.slots, Some(Slots { total: 3, working: 3 }));
        assert_eq!(waiting.members.last().unwrap().state, MemberState::Spare);
        let sync = waiting.sync.as_ref().unwrap();
        assert_eq!((sync.operation, sync.percent), (SyncOperation::Resync, None));
    }

    #[test]
    fn no_arrays() {
        assert_eq!(parse("").unwrap(), []);
        assert_eq!(parse("Personalities : \nunused devices: <none>\n").unwrap(), []);
        assert!(parse("md0 : broken\n").is_err());
    }
}
