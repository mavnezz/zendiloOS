//! The simulated NAS: disks with partitions and ZFS pools, rendered as the
//! real tools print them (`lsblk --json`, `zpool status -j`, `zfs list -j`),
//! so zendiloosd's parsers run against it unchanged.

use serde_json::{Value, json};

/// Seconds a simulated scrub or resilver takes.
pub(crate) const SCAN_SECONDS: u64 = 90;

#[derive(Debug, Clone)]
pub(crate) struct Disk {
    pub name: String,
    pub model: String,
    pub serial: String,
    pub size: u64,
    pub transport: &'static str,
    pub rotational: bool,
    pub partitions: Vec<Partition>,
}

#[derive(Debug, Clone)]
pub(crate) struct Partition {
    pub size: u64,
    pub fstype: Option<&'static str>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    pub mount: Option<&'static str>,
    /// Filesystem size and use while mounted.
    pub usage: Option<(u64, u64)>,
}

impl Disk {
    pub fn id_link(&self) -> String {
        let prefix = if self.transport == "nvme" { "nvme" } else { "ata" };
        format!("{prefix}-{}_{}", self.model.replace(' ', "_"), self.serial)
    }

    fn partition_name(&self, number: usize) -> String {
        if self.name.ends_with(|c: char| c.is_ascii_digit()) {
            format!("{}p{number}", self.name)
        } else {
            format!("{}{number}", self.name)
        }
    }

    /// The by-id path ZFS uses for the disk's first partition.
    pub fn zfs_path(&self) -> String {
        format!("/dev/disk/by-id/{}-part1", self.id_link())
    }

    fn lsblk(&self) -> Value {
        let children: Vec<Value> = self
            .partitions
            .iter()
            .enumerate()
            .map(|(index, partition)| {
                let name = self.partition_name(index + 1);
                json!({
                    "name": name, "kname": name, "path": format!("/dev/{name}"), "type": "part",
                    "size": partition.size, "model": null, "serial": null, "wwn": null,
                    "tran": self.transport, "rota": self.rotational, "rm": false, "ro": false,
                    "fstype": partition.fstype, "label": partition.label, "uuid": partition.uuid,
                    "fssize": partition.usage.map(|u| u.0), "fsused": partition.usage.map(|u| u.1),
                    "fsavail": partition.usage.map(|u| u.0 - u.1),
                    "mountpoints": partition.mount.map_or_else(Vec::new, |m| vec![m]),
                    "pkname": self.name, "id-link": format!("{}-part{}", self.id_link(), index + 1),
                })
            })
            .collect();
        json!({
            "name": self.name, "kname": self.name, "path": format!("/dev/{}", self.name), "type": "disk",
            "size": self.size, "model": self.model, "serial": self.serial, "wwn": null,
            "tran": self.transport, "rota": self.rotational, "rm": false, "ro": false,
            "fstype": null, "label": null, "uuid": null, "fssize": null, "fsused": null, "fsavail": null,
            "mountpoints": [], "pkname": null, "id-link": self.id_link(), "children": children,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    Single,
    Mirror,
    Raidz(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberState {
    Online,
    Faulted,
    Removed,
}

impl MemberState {
    fn as_str(self) -> &'static str {
        match self {
            MemberState::Online => "ONLINE",
            MemberState::Faulted => "FAULTED",
            MemberState::Removed => "REMOVED",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Member {
    /// By-id path, which stays even when the disk is gone.
    pub path: String,
    pub guid: u64,
    pub state: MemberState,
    pub read_errors: u64,
    /// The device resilvering in to replace this one.
    pub replacement: Option<Box<Member>>,
}

impl Member {
    pub fn new(path: String, guid: u64) -> Self {
        Self { path, guid, state: MemberState::Online, read_errors: 0, replacement: None }
    }

    fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    fn leaf(&self) -> Value {
        json!({
            "name": self.name(), "vdev_type": "disk", "guid": self.guid, "path": self.path,
            "class": "normal", "state": self.state.as_str(),
            "read_errors": self.read_errors, "write_errors": 0, "checksum_errors": 0,
        })
    }

    fn vdev(&self, index: usize, state: &str) -> (String, Value) {
        match &self.replacement {
            None => (self.name().to_owned(), self.leaf()),
            Some(new) => {
                let mut children = serde_json::Map::new();
                children.insert(self.name().to_owned(), self.leaf());
                children.insert(new.name().to_owned(), new.leaf());
                let name = format!("replacing-{index}");
                let vdev = json!({
                    "name": name, "vdev_type": "replacing", "guid": self.guid ^ 0x5eed, "class": "normal",
                    "state": state, "read_errors": 0, "write_errors": 0, "checksum_errors": 0,
                    "vdevs": children,
                });
                (name, vdev)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanKind {
    Scrub,
    Resilver,
}

#[derive(Debug, Clone)]
pub(crate) struct ScanRun {
    pub kind: ScanKind,
    pub started_at: u64,
    /// When it was stopped, for a stopped scrub.
    pub stopped_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct Pool {
    pub name: String,
    pub guid: u64,
    pub layout: Layout,
    pub members: Vec<Member>,
    pub volume: Option<u16>,
    /// Capacity of one member.
    pub member_size: u64,
    pub used: u64,
    pub scan: Option<ScanRun>,
    /// Datasets of shares below the root dataset.
    pub shares: Vec<String>,
    pub snapshots: Vec<SimSnapshot>,
}

/// A snapshot of a share, as `zfs list -t snapshot` would show it.
#[derive(Debug, Clone)]
pub(crate) struct SimSnapshot {
    pub share: String,
    pub label: String,
    pub created: u64,
    /// `hourly,daily` or `-`.
    pub schedules: String,
    /// The note or `-`.
    pub note: String,
}

impl Pool {
    fn redundancy(&self) -> usize {
        match self.layout {
            Layout::Single => 0,
            Layout::Mirror => self.members.len() - 1,
            Layout::Raidz(parity) => usize::from(parity),
        }
    }

    fn data_disks(&self) -> u64 {
        match self.layout {
            Layout::Single => 1,
            Layout::Mirror => 1,
            Layout::Raidz(parity) => self.members.len() as u64 - u64::from(parity),
        }
    }

    fn size(&self) -> u64 {
        self.member_size * self.data_disks()
    }

    fn state(&self) -> &'static str {
        let failed = self.members.iter().filter(|m| m.state != MemberState::Online).count();
        if failed > self.redundancy() {
            "UNAVAIL"
        } else if failed > 0 || self.members.iter().any(|m| m.replacement.is_some()) {
            "DEGRADED"
        } else {
            "ONLINE"
        }
    }

    /// Completes a finished resilver: replaced devices leave the pool.
    pub fn settle(&mut self, now: u64) {
        let finished = self.scan.as_ref().is_some_and(|scan| now >= scan.started_at + SCAN_SECONDS);
        if !finished || self.scan.as_ref().is_some_and(|scan| scan.kind != ScanKind::Resilver) {
            return;
        }
        for member in &mut self.members {
            if let Some(new) = member.replacement.take() {
                *member = *new;
            }
        }
    }

    fn scan_stats(&self, now: u64) -> Option<Value> {
        let scan = self.scan.as_ref()?;
        let total = self.used.max(1);
        let until = scan.stopped_at.unwrap_or(now);
        let elapsed = until.saturating_sub(scan.started_at);
        let finished = scan.stopped_at.is_none() && elapsed >= SCAN_SECONDS;
        let issued = if finished { total } else { total / SCAN_SECONDS * elapsed.min(SCAN_SECONDS) };
        let (state, end_time) = match (scan.stopped_at, finished) {
            (Some(stopped), _) => ("CANCELED", stopped),
            (None, true) => ("FINISHED", scan.started_at + SCAN_SECONDS),
            (None, false) => ("SCANNING", 0),
        };
        Some(json!({
            "function": match scan.kind { ScanKind::Scrub => "SCRUB", ScanKind::Resilver => "RESILVER" },
            "state": state,
            "start_time": scan.started_at,
            "end_time": end_time,
            "to_examine": total, "examined": issued, "issued": issued, "errors": 0,
            "pass_start": scan.started_at, "issued_bytes_per_scan": issued,
            "scrub_pause": 0, "scrub_spent_paused": 0,
        }))
    }

    fn status(&self, now: u64) -> Value {
        let state = self.state();
        let mut leaves = serde_json::Map::new();
        for (index, member) in self.members.iter().enumerate() {
            let (name, vdev) = member.vdev(index, state);
            leaves.insert(name, vdev);
        }
        let errors = json!({ "read_errors": 0, "write_errors": 0, "checksum_errors": 0 });
        let data = match self.layout {
            Layout::Single => leaves,
            Layout::Mirror | Layout::Raidz(_) => {
                let (name, kind) = match self.layout {
                    Layout::Mirror => ("mirror-0".to_owned(), "mirror"),
                    Layout::Raidz(parity) => (format!("raidz{parity}-0"), "raidz"),
                    Layout::Single => unreachable!(),
                };
                let mut vdev = json!({
                    "name": name, "vdev_type": kind, "guid": self.guid ^ 0xabc, "class": "normal",
                    "state": state, "vdevs": leaves,
                });
                merge(&mut vdev, &errors);
                let mut map = serde_json::Map::new();
                map.insert(name, vdev);
                map
            }
        };
        let mut root = json!({
            "name": self.name, "vdev_type": "root", "guid": self.guid, "class": "normal", "state": state,
            "alloc_space": self.used, "total_space": self.size(), "vdevs": data,
        });
        merge(&mut root, &errors);
        let mut pool = json!({
            "name": self.name, "state": state, "pool_guid": self.guid, "error_count": 0,
            "vdevs": { self.name.clone(): root },
        });
        if let Some(stats) = self.scan_stats(now) {
            pool["scan_stats"] = stats;
        }
        pool
    }

    fn dataset(&self) -> Value {
        // Roughly what ZFS reports: a little less than the raw capacity.
        let available = (self.size() / 100 * 96).saturating_sub(self.used);
        json!({
            "name": self.name, "type": "FILESYSTEM", "pool": self.name,
            "properties": {
                "used": { "value": self.used }, "available": { "value": available },
                "mounted": { "value": if self.volume.is_some() { "yes" } else { "no" } },
                "zendiloos:volume": { "value": self.volume.map_or(json!("-"), |n| json!(n)) },
            },
        })
    }
}

fn merge(target: &mut Value, extra: &Value) {
    if let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
}

const TB4: u64 = 4_000_787_030_016;
const GIB: u64 = 1024 * 1024 * 1024;

/// The whole simulated machine.
#[derive(Debug, Clone)]
pub(crate) struct Host {
    pub disks: Vec<Disk>,
    pub pools: Vec<Pool>,
}

impl Host {
    /// A system NVMe; a healthy mirror and a degraded mirror (one disk
    /// failing); a sleeping disk and an unused NVMe.
    pub fn nas() -> Self {
        let hdd = |name: &str, serial: &str| Disk {
            name: name.to_owned(),
            model: "SIM HDD 4TB".to_owned(),
            serial: serial.to_owned(),
            size: TB4,
            transport: "sata",
            rotational: true,
            partitions: Vec::new(),
        };
        let system = Disk {
            name: "nvme0n1".to_owned(),
            model: "SIM NVMe SSD 512GB".to_owned(),
            serial: "SIMNV0001".to_owned(),
            size: 512_110_190_592,
            transport: "nvme",
            rotational: false,
            partitions: vec![
                Partition {
                    size: 512 * 1024 * 1024,
                    fstype: Some("vfat"),
                    label: None,
                    uuid: Some("4A1B-2C3D".to_owned()),
                    mount: Some("/boot/efi"),
                    usage: Some((536_608_768, 6_291_456)),
                },
                Partition {
                    size: 494 * GIB,
                    fstype: Some("ext4"),
                    label: None,
                    uuid: Some("3c5d7e9f-1a2b-4c3d-8e4f-5a6b7c8d9e0f".to_owned()),
                    mount: Some("/"),
                    usage: Some((494 * GIB, 9 * GIB)),
                },
                Partition {
                    size: 8 * GIB,
                    fstype: Some("swap"),
                    label: None,
                    uuid: Some("9f8e7d6c-5b4a-4321-8fed-cba987654321".to_owned()),
                    mount: Some("[SWAP]"),
                    usage: None,
                },
            ],
        };
        let spare = Disk {
            name: "nvme1n1".to_owned(),
            model: "SIM NVMe SSD 1TB".to_owned(),
            serial: "SIMNV0002".to_owned(),
            size: 1_000_204_886_016,
            transport: "nvme",
            rotational: false,
            partitions: Vec::new(),
        };
        let mut host = Self {
            disks: vec![
                hdd("sda", "SIM4T0001"),
                hdd("sdb", "SIM4T0002"),
                hdd("sdc", "SIM4T0003"),
                hdd("sdd", "SIM4T0004"),
                hdd("sde", "SIM4T0005"),
                system,
                spare,
            ],
            pools: Vec::new(),
        };
        host.add_pool("tank", Layout::Mirror, &["sda", "sdb"], Some(1), 1_310 * GIB, 0x88a1_3c2e_5b0a_4c8e);
        host.add_pool("backup", Layout::Mirror, &["sdc", "sdd"], Some(2), 2_480 * GIB, 0x6f1d_3c2e_9d1f_2a7b);
        let failing = &mut host.pools[1].members[1];
        failing.state = MemberState::Faulted;
        failing.read_errors = 1_736;
        host
    }

    /// Partitions `disks` for ZFS and creates the pool on them.
    pub fn add_pool(
        &mut self,
        name: &str,
        layout: Layout,
        disks: &[&str],
        volume: Option<u16>,
        used: u64,
        guid: u64,
    ) {
        let mut members = Vec::new();
        let mut member_size = u64::MAX;
        for (index, disk_name) in disks.iter().enumerate() {
            let disk = self.disks.iter_mut().find(|disk| disk.name == *disk_name).expect("known disk");
            let size = disk.size - 129 * 1024 * 1024;
            disk.partitions = vec![Partition {
                size,
                fstype: Some("zfs_member"),
                label: Some(name.to_owned()),
                uuid: Some(guid.to_string()),
                mount: None,
                usage: None,
            }];
            member_size = member_size.min(size);
            members
                .push(Member::new(disk.zfs_path(), guid.wrapping_add(index as u64 + 1).wrapping_mul(7919)));
        }
        self.pools.push(Pool {
            name: name.to_owned(),
            guid,
            layout,
            members,
            volume,
            member_size,
            used,
            scan: None,
            shares: Vec::new(),
            snapshots: Vec::new(),
        });
    }

    pub fn lsblk(&self) -> String {
        let devices: Vec<Value> = self.disks.iter().map(Disk::lsblk).collect();
        json!({ "blockdevices": devices }).to_string()
    }

    pub fn zpool_status(&mut self, now: u64) -> String {
        let mut pools = serde_json::Map::new();
        for pool in &mut self.pools {
            pool.settle(now);
            pools.insert(pool.name.clone(), pool.status(now));
        }
        json!({
            "output_version": { "command": "zpool status", "vers_major": 0, "vers_minor": 1 },
            "pools": pools,
        })
        .to_string()
    }

    pub fn zfs_list(&self) -> String {
        let datasets: serde_json::Map<String, Value> =
            self.pools.iter().map(|pool| (pool.name.clone(), pool.dataset())).collect();
        json!({
            "output_version": { "command": "zfs list", "vers_major": 0, "vers_minor": 1 },
            "datasets": datasets,
        })
        .to_string()
    }
}

/// Why the simulated host refused a change.
#[derive(Debug)]
pub(crate) enum Refusal {
    Refused(String),
    Conflict,
}

impl Host {
    fn disk_by_id(&self, id: &str) -> Result<usize, Refusal> {
        self.disks
            .iter()
            .position(|disk| disk.id_link() == id)
            .ok_or_else(|| Refusal::Refused(format!("disk {id} is not present")))
    }

    /// Like the agent: only disks without partitions in use.
    fn check_free(&self, index: usize) -> Result<(), Refusal> {
        let disk = &self.disks[index];
        let path = disk.zfs_path();
        let in_pool = self.pools.iter().flat_map(|p| &p.members).any(|m| {
            m.path == path && m.state != MemberState::Removed
                || m.replacement.as_ref().is_some_and(|r| r.path == path)
        });
        if in_pool || disk.partitions.iter().any(|p| p.mount.is_some()) {
            return Err(Refusal::Refused(format!("disk {} is in use", disk.name)));
        }
        Ok(())
    }

    pub fn create(&mut self, name: &str, layout: Layout, ids: &[&str], volume: u16) -> Result<(), Refusal> {
        let mut names = Vec::new();
        for id in ids {
            let index = self.disk_by_id(id)?;
            self.check_free(index)?;
            names.push(self.disks[index].name.clone());
        }
        if self.pools.iter().any(|p| p.name == name || p.volume == Some(volume)) {
            return Err(Refusal::Conflict);
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let guid = 0x1000_0000_0000_u64 + u64::from(volume) * 7919 + self.pools.len() as u64;
        self.add_pool(name, layout, &names, Some(volume), 0, guid);
        Ok(())
    }

    pub fn destroy(&mut self, name: &str, volume: u16) -> Result<(), Refusal> {
        let index = self
            .pools
            .iter()
            .position(|p| p.name == name && p.volume == Some(volume))
            .ok_or_else(|| Refusal::Refused(format!("pool {name} is not managed by zendiloos")))?;
        let pool = self.pools.remove(index);
        for member in &pool.members {
            if let Some(disk) = self.disks.iter_mut().find(|d| d.zfs_path() == member.path) {
                disk.partitions.clear();
            }
        }
        Ok(())
    }

    pub fn replace(&mut self, pool: &str, device: u64, id: &str, now: u64) -> Result<(), Refusal> {
        let index = self.disk_by_id(id)?;
        self.check_free(index)?;
        let pool_index = self
            .pools
            .iter()
            .position(|p| p.name == pool && p.volume.is_some())
            .ok_or_else(|| Refusal::Refused(format!("pool {pool} is not managed by zendiloos")))?;
        if !self.pools[pool_index].members.iter().any(|m| m.guid == device) {
            return Err(Refusal::Refused(format!("pool {pool} has no device {device}")));
        }
        let disk = &mut self.disks[index];
        let size = self.pools[pool_index].member_size;
        disk.partitions = vec![Partition {
            size,
            fstype: Some("zfs_member"),
            label: Some(pool.to_owned()),
            uuid: Some(self.pools[pool_index].guid.to_string()),
            mount: None,
            usage: None,
        }];
        let path = disk.zfs_path();
        let pool = &mut self.pools[pool_index];
        let member = pool.members.iter_mut().find(|m| m.guid == device).expect("checked above");
        member.replacement = Some(Box::new(Member::new(path, device.wrapping_mul(31).wrapping_add(17))));
        pool.scan = Some(ScanRun { kind: ScanKind::Resilver, started_at: now, stopped_at: None });
        Ok(())
    }

    fn managed_pool(&mut self, pool: &str, volume: u16) -> Result<&mut Pool, Refusal> {
        self.pools
            .iter_mut()
            .find(|p| p.name == pool && p.volume == Some(volume))
            .ok_or_else(|| Refusal::Refused(format!("pool {pool} is not managed by zendiloos")))
    }

    pub fn create_share(&mut self, pool: &str, volume: u16, name: &str) -> Result<(), Refusal> {
        let pool = self.managed_pool(pool, volume)?;
        if pool.shares.iter().any(|share| share == name) {
            return Err(Refusal::Conflict);
        }
        pool.shares.push(name.to_owned());
        Ok(())
    }

    pub fn destroy_share(&mut self, pool: &str, volume: u16, name: &str) -> Result<(), Refusal> {
        self.managed_pool(pool, volume)?.shares.retain(|share| share != name);
        Ok(())
    }

    pub fn create_snapshot(
        &mut self,
        pool: &str,
        volume: u16,
        share: &str,
        snapshot: SimSnapshot,
    ) -> Result<(), Refusal> {
        let pool = self.managed_pool(pool, volume)?;
        if !pool.shares.iter().any(|s| s == share) {
            return Err(Refusal::Refused(format!("there is no share {}/{share}", pool.name)));
        }
        if pool.snapshots.iter().any(|s| s.share == share && s.label == snapshot.label) {
            return Err(Refusal::Conflict);
        }
        pool.snapshots.push(snapshot);
        Ok(())
    }

    pub fn destroy_snapshot(
        &mut self,
        pool: &str,
        volume: u16,
        share: &str,
        label: &str,
    ) -> Result<(), Refusal> {
        let pool = self.managed_pool(pool, volume)?;
        let before = pool.snapshots.len();
        pool.snapshots.retain(|s| !(s.share == share && s.label == label));
        if pool.snapshots.len() == before {
            return Err(Refusal::Refused(format!("there is no snapshot {}/{share}@{label}", pool.name)));
        }
        Ok(())
    }

    /// Like `zfs list -H -p -t snapshot -o name,creation,used,referenced,…`.
    pub fn snapshot_list(&self) -> String {
        let mut text = String::new();
        for pool in &self.pools {
            for s in &pool.snapshots {
                text.push_str(&format!(
                    "{}/{}@{}\t{}\t{}\t{}\t{}\t{}\n",
                    pool.name, s.share, s.label, s.created, 0, pool.used, s.schedules, s.note
                ));
            }
        }
        text
    }

    /// Like the files agent: only a share that is there.
    pub fn share_present(&self, volume: u16, name: &str) -> Result<(), Refusal> {
        let present =
            self.pools.iter().any(|p| p.volume == Some(volume) && p.shares.iter().any(|s| s == name));
        if present { Ok(()) } else { Err(Refusal::Refused(format!("share {name} is not mounted"))) }
    }

    pub fn scrub(&mut self, pool: &str, now: u64) -> Result<(), Refusal> {
        let pool = self
            .pools
            .iter_mut()
            .find(|p| p.name == pool)
            .ok_or_else(|| Refusal::Refused(format!("there is no pool {pool}")))?;
        pool.scan = Some(ScanRun { kind: ScanKind::Scrub, started_at: now, stopped_at: None });
        Ok(())
    }

    /// Like `zpool scrub -s`: only a running scrub.
    pub fn stop_scrub(&mut self, pool: &str, now: u64) -> Result<(), Refusal> {
        let pool = self
            .pools
            .iter_mut()
            .find(|p| p.name == pool)
            .ok_or_else(|| Refusal::Refused(format!("there is no pool {pool}")))?;
        let running = pool.scan.as_mut().filter(|scan| {
            scan.kind == ScanKind::Scrub && scan.stopped_at.is_none() && now < scan.started_at + SCAN_SECONDS
        });
        let Some(scan) = running else {
            return Err(Refusal::Refused(format!("there is no scrub in progress on {}", pool.name)));
        };
        scan.stopped_at = Some(now);
        Ok(())
    }
}
