//! What is worth telling an administrator about storage: changes between
//! two looks at the pools and disks.

use std::collections::HashMap;

use super::pool::{Pool, ScanKind, ScanState, VdevState};
use super::smart::Health;

/// Fill levels at which a volume is reported, in percent.
pub const FILL_THRESHOLDS: [u8; 2] = [80, 90];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageEvent {
    /// The pool lost redundancy or its data became unavailable.
    PoolTrouble {
        pool: String,
        state: VdevState,
    },
    /// The pool is healthy again.
    PoolRecovered {
        pool: String,
    },
    ScanFinished {
        pool: String,
        kind: ScanKind,
        errors: u64,
    },
    VolumeFilling {
        pool: String,
        percent: u8,
    },
    DiskHealth {
        disk: String,
        model: Option<String>,
        health: Health,
    },
}

pub(crate) fn percent_used(pool: &Pool) -> Option<u8> {
    let space = pool.space?;
    let total = space.used_bytes + space.available_bytes;
    (total > 0).then(|| (space.used_bytes.saturating_mul(100) / total).min(100) as u8)
}

fn threshold(percent: u8) -> Option<u8> {
    FILL_THRESHOLDS.iter().rev().copied().find(|&t| percent >= t)
}

/// Events between `before` (absent on the first look) and `after`. On the
/// first look only problems are reported, so that a restart reminds of
/// them but does not repeat finished scans.
pub fn pool_events(before: Option<&[Pool]>, after: &[Pool]) -> Vec<StorageEvent> {
    let previous: HashMap<&str, &Pool> =
        before.unwrap_or_default().iter().map(|p| (p.name.as_str(), p)).collect();
    let mut events = Vec::new();
    for pool in after {
        let earlier = previous.get(pool.name.as_str()).copied();
        let was = earlier.map_or(VdevState::Online, |p| p.state);
        if pool.state != was {
            if pool.state == VdevState::Online {
                events.push(StorageEvent::PoolRecovered { pool: pool.name.clone() });
            } else {
                events.push(StorageEvent::PoolTrouble { pool: pool.name.clone(), state: pool.state });
            }
        }
        if let (Some(scan), Some(earlier)) = (&pool.scan, earlier)
            && scan.state == ScanState::Finished
        {
            let seen = earlier.scan.as_ref();
            let was_running = seen.is_some_and(|s| s.state == ScanState::Running && s.kind == scan.kind);
            // A small pool's scrub starts and ends between two looks.
            let whole_since = seen.is_none_or(|s| s.started_at < scan.started_at);
            if was_running || whole_since {
                events.push(StorageEvent::ScanFinished {
                    pool: pool.name.clone(),
                    kind: scan.kind,
                    errors: scan.errors,
                });
            }
        }
        let now = percent_used(pool).and_then(threshold);
        let then = earlier.and_then(percent_used).and_then(threshold);
        if let Some(level) = now
            && then.is_none_or(|then| level > then)
        {
            events.push(StorageEvent::VolumeFilling { pool: pool.name.clone(), percent: level });
        }
    }
    events
}

/// A disk whose SMART verdict got worse than `before` (absent: good).
pub fn disk_event(
    disk: &str,
    model: Option<&str>,
    before: Option<Health>,
    after: Health,
) -> Option<StorageEvent> {
    (after > before.unwrap_or(Health::Good)).then(|| StorageEvent::DiskHealth {
        disk: disk.to_owned(),
        model: model.map(str::to_owned),
        health: after,
    })
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::domain::pool::{PoolLayout, PoolSpace, Scan};

    fn pool(state: VdevState, scan: Option<ScanState>, used: u64) -> Pool {
        Pool {
            name: "tank".into(),
            guid: "1".into(),
            state,
            layout: PoolLayout::Mirror,
            devices: vec![],
            size_bytes: 100,
            allocated_bytes: used,
            scan: scan.map(|state| Scan {
                kind: ScanKind::Resilver,
                state,
                done_bytes: 0,
                total_bytes: 1,
                started_at: Timestamp::from_unix_seconds(0),
                finished_at: None,
                errors: 0,
                pass: None,
            }),
            data_errors: 0,
            volume: None,
            space: Some(PoolSpace { used_bytes: used, available_bytes: 100 - used, mounted: true }),
        }
    }

    #[test]
    fn reports_state_changes_and_finished_scans() {
        let degraded = pool(VdevState::Degraded, Some(ScanState::Running), 10);
        let healthy = pool(VdevState::Online, Some(ScanState::Finished), 10);
        assert_eq!(
            pool_events(Some(std::slice::from_ref(&degraded)), &[healthy]),
            [
                StorageEvent::PoolRecovered { pool: "tank".into() },
                StorageEvent::ScanFinished { pool: "tank".into(), kind: ScanKind::Resilver, errors: 0 },
            ]
        );
        let online = pool(VdevState::Online, None, 10);
        assert_eq!(
            pool_events(Some(&[online]), std::slice::from_ref(&degraded)),
            [StorageEvent::PoolTrouble { pool: "tank".into(), state: VdevState::Degraded }]
        );
        assert_eq!(
            pool_events(Some(std::slice::from_ref(&degraded)), std::slice::from_ref(&degraded)),
            [],
            "nothing new"
        );
    }

    #[test]
    fn a_scan_that_ran_between_two_looks_is_reported_once() {
        let started = |seconds, state| {
            let mut pool = pool(VdevState::Online, Some(state), 10);
            if let Some(scan) = &mut pool.scan {
                (scan.kind, scan.started_at) = (ScanKind::Scrub, Timestamp::from_unix_seconds(seconds));
            }
            pool
        };
        let finished = StorageEvent::ScanFinished { pool: "tank".into(), kind: ScanKind::Scrub, errors: 0 };
        let never = pool(VdevState::Online, None, 10);
        let done = started(100, ScanState::Finished);
        assert_eq!(pool_events(Some(&[never]), std::slice::from_ref(&done)), std::slice::from_ref(&finished));
        let earlier = started(0, ScanState::Finished);
        assert_eq!(pool_events(Some(&[earlier]), std::slice::from_ref(&done)), [finished]);
        assert_eq!(
            pool_events(Some(std::slice::from_ref(&done)), std::slice::from_ref(&done)),
            [],
            "seen already"
        );
    }

    #[test]
    fn the_first_look_reports_only_problems() {
        let finished = pool(VdevState::Online, Some(ScanState::Finished), 10);
        assert_eq!(pool_events(None, &[finished]), []);
        let faulted = pool(VdevState::Faulted, None, 95);
        assert_eq!(
            pool_events(None, &[faulted]),
            [
                StorageEvent::PoolTrouble { pool: "tank".into(), state: VdevState::Faulted },
                StorageEvent::VolumeFilling { pool: "tank".into(), percent: 90 },
            ]
        );
    }

    #[test]
    fn reports_each_fill_level_once() {
        let at = |used| pool(VdevState::Online, None, used);
        assert_eq!(
            pool_events(Some(&[at(70)]), &[at(85)]),
            [StorageEvent::VolumeFilling { pool: "tank".into(), percent: 80 }]
        );
        assert_eq!(pool_events(Some(&[at(85)]), &[at(88)]), []);
        assert_eq!(
            pool_events(Some(&[at(88)]), &[at(91)]),
            [StorageEvent::VolumeFilling { pool: "tank".into(), percent: 90 }]
        );
        assert_eq!(pool_events(Some(&[at(91)]), &[at(60)]), []);
    }

    #[test]
    fn reports_disks_getting_worse() {
        assert!(disk_event("sda", None, None, Health::Good).is_none());
        assert!(disk_event("sda", None, Some(Health::Warning), Health::Warning).is_none());
        assert!(disk_event("sda", None, Some(Health::Failing), Health::Warning).is_none());
        let event = disk_event("sdd", Some("HDD"), Some(Health::Warning), Health::Failing).unwrap();
        assert_eq!(
            event,
            StorageEvent::DiskHealth {
                disk: "sdd".into(),
                model: Some("HDD".into()),
                health: Health::Failing
            }
        );
    }
}
