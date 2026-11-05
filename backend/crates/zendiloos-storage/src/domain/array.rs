/// A Linux software RAID array (md).
#[derive(Debug, Clone, PartialEq)]
pub struct Array {
    /// Kernel name, e.g. `md0`.
    pub name: String,
    /// `raid1`, `raid5`, `linear`, …; unknown while inactive.
    pub level: Option<String>,
    pub active: bool,
    pub read_only: bool,
    pub size_bytes: Option<u64>,
    /// Absent for levels without redundancy (`raid0`, `linear`).
    pub slots: Option<Slots>,
    pub members: Vec<ArrayMember>,
    /// A running or waiting resync, recovery, check or reshape.
    pub sync: Option<SyncProgress>,
}

/// Devices the array is made for, and how many of them work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slots {
    pub total: u32,
    pub working: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayMember {
    /// Kernel name of the disk or partition, e.g. `sdb` or `sdb1`.
    pub device: String,
    /// Position in the array's superblock.
    pub index: u32,
    pub state: MemberState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberState {
    Active,
    Spare,
    Faulty,
    /// Being copied onto, to replace another member.
    Replacement,
    /// A write journal (RAID 4/5/6).
    Journal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncProgress {
    pub operation: SyncOperation,
    /// 0 to 100; `None` while waiting.
    pub percent: Option<f32>,
    pub remaining_seconds: Option<u64>,
    pub speed_bytes_per_second: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOperation {
    /// Making the copies consistent, e.g. after creation or a crash.
    Resync,
    /// Rebuilding onto a new member.
    Recovery,
    /// A scrub that only reads and counts mismatches.
    Check,
    /// A scrub that also repairs.
    Repair,
    Reshape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayHealth {
    Healthy,
    /// Redundancy is reduced, and nothing is rebuilding it.
    Degraded,
    /// Redundancy is reduced and being rebuilt.
    Rebuilding,
    /// Assembled but not running; its data is unavailable.
    Inactive,
}

impl Array {
    pub fn health(&self) -> ArrayHealth {
        if !self.active {
            return ArrayHealth::Inactive;
        }
        match self.slots {
            Some(Slots { total, working }) if working < total => {
                let rebuilding =
                    self.sync.as_ref().is_some_and(|sync| sync.operation == SyncOperation::Recovery);
                if rebuilding { ArrayHealth::Rebuilding } else { ArrayHealth::Degraded }
            }
            _ => ArrayHealth::Healthy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array(slots: Option<(u32, u32)>, sync: Option<SyncOperation>) -> Array {
        Array {
            name: "md0".into(),
            level: Some("raid1".into()),
            active: true,
            read_only: false,
            size_bytes: Some(8 << 30),
            slots: slots.map(|(total, working)| Slots { total, working }),
            members: Vec::new(),
            sync: sync.map(|operation| SyncProgress {
                operation,
                percent: Some(2.4),
                remaining_seconds: None,
                speed_bytes_per_second: None,
            }),
        }
    }

    #[test]
    fn health_follows_slots_and_sync() {
        assert_eq!(array(Some((2, 2)), None).health(), ArrayHealth::Healthy);
        assert_eq!(array(Some((2, 2)), Some(SyncOperation::Resync)).health(), ArrayHealth::Healthy);
        assert_eq!(array(Some((2, 1)), None).health(), ArrayHealth::Degraded);
        assert_eq!(array(Some((2, 1)), Some(SyncOperation::Check)).health(), ArrayHealth::Degraded);
        assert_eq!(array(Some((2, 1)), Some(SyncOperation::Recovery)).health(), ArrayHealth::Rebuilding);
        assert_eq!(array(None, None).health(), ArrayHealth::Healthy, "raid0 has no redundancy to lose");
        assert_eq!(Array { active: false, ..array(Some((2, 2)), None) }.health(), ArrayHealth::Inactive);
    }
}
