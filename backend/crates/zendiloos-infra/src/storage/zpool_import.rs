//! `zpool import` without a pool: what it finds on the disks that is not
//! imported, as text (OpenZFS 2.3 has no JSON for it). A block per pool,
//! `pool:`, `id:`, `state:`, `status:` and `action:` (continued on lines
//! of their own), `see:`, then `config:` and the devices as a tree.

use zendiloos_storage::domain::ImportablePool;

use super::ParseError;

/// Labels `zpool import` puts before a value.
const KEYS: [&str; 8] = ["pool", "id", "state", "status", "action", "see", "config", "comment"];

/// The vdevs that group devices rather than being one.
fn is_group(name: &str) -> bool {
    let kind = name.split_once('-').map_or(name, |(kind, _)| kind);
    matches!(kind, "mirror" | "raidz1" | "raidz2" | "raidz3" | "raidz" | "replacing" | "spare")
        || kind.starts_with("draid")
        || matches!(name, "logs" | "cache" | "spares" | "special" | "dedup")
}

fn key_of(line: &str) -> Option<(&'static str, &str)> {
    let (key, value) = line.split_once(':')?;
    KEYS.iter().find(|known| **known == key).map(|known| (*known, value.trim()))
}

#[derive(Default)]
struct Block {
    name: String,
    id: Option<u64>,
    state: String,
    status: Option<String>,
    action: Option<String>,
    devices: Vec<(String, String)>,
}

impl Block {
    fn finish(self) -> Result<ImportablePool, ParseError> {
        let id =
            self.id.ok_or_else(|| ParseError::new("zpool import", format!("{} has no id", self.name)))?;
        let action = self.action;
        let status = self.status;
        let said =
            |text: &Option<String>, what: &str| text.as_deref().is_some_and(|text| text.contains(what));
        let other_host = said(&status, "another system") || said(&action, "'-f'");
        let can_import =
            !matches!(self.state.as_str(), "UNAVAIL" | "FAULTED") && !said(&action, "cannot be imported");
        Ok(ImportablePool {
            name: self.name,
            id,
            state: self.state,
            other_host,
            can_import,
            status,
            action,
            devices: self.devices,
        })
    }
}

/// The pools in `text`, in its order.
pub(crate) fn parse(text: &str) -> Result<Vec<ImportablePool>, ParseError> {
    let mut pools = Vec::new();
    let mut block: Option<Block> = None;
    let mut last: Option<&'static str> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let in_config = last == Some("config");
        if let Some((key, value)) = key_of(line).filter(|_| !in_config || line.starts_with("pool:")) {
            if key == "pool" {
                if let Some(done) = block.take() {
                    pools.push(done.finish()?);
                }
                block = Some(Block { name: value.to_owned(), ..Block::default() });
                last = Some(key);
                continue;
            }
            let current = block
                .as_mut()
                .ok_or_else(|| ParseError::new("zpool import", format!("{key} before any pool")))?;
            match key {
                "id" => {
                    current.id = Some(
                        value.parse().map_err(|_| ParseError::new("zpool import", format!("id {value}")))?,
                    );
                }
                "state" => value.clone_into(&mut current.state),
                "status" => current.status = Some(value.to_owned()),
                "action" => current.action = Some(value.to_owned()),
                _ => {}
            }
            last = Some(key);
            continue;
        }
        let Some(current) = block.as_mut() else { continue };
        match last {
            Some("config") => {
                let mut words = line.split_whitespace();
                let (Some(name), Some(state)) = (words.next(), words.next()) else { continue };
                if name != current.name && !is_group(name) {
                    current.devices.push((name.to_owned(), state.to_owned()));
                }
            }
            Some("status") => append(&mut current.status, line),
            Some("action") => append(&mut current.action, line),
            _ => {}
        }
    }
    if let Some(done) = block {
        pools.push(done.finish()?);
    }
    Ok(pools)
}

fn append(text: &mut Option<String>, line: &str) {
    if let Some(text) = text {
        text.push(' ');
        text.push_str(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_zpool_import_finds() {
        let pools = parse(include_str!("fixtures/zpool-import-three-pools.txt")).unwrap();
        let names: Vec<&str> = pools.iter().map(|pool| pool.name.as_str()).collect();
        assert_eq!(names, ["tank", "cold", "gone"]);

        let tank = &pools[0];
        assert_eq!(tank.id, 15_378_431_232_141_542_312);
        assert_eq!(tank.state, "ONLINE");
        assert!(tank.other_host && tank.can_import);
        assert_eq!(tank.status.as_deref(), Some("The pool was last accessed by another system."));
        assert_eq!(
            tank.action.as_deref(),
            Some("The pool can be imported using its name or numeric identifier and the '-f' flag.")
        );
        assert_eq!(
            tank.devices,
            [
                ("ata-QEMU_HARDDISK_QM00007-part1".to_owned(), "ONLINE".to_owned()),
                ("ata-QEMU_HARDDISK_QM00009-part1".to_owned(), "ONLINE".to_owned()),
            ]
        );

        let cold = &pools[1];
        assert!(cold.can_import && !cold.other_host, "degraded, but importable here");
        assert_eq!(cold.devices[1], ("1849237146892741233".to_owned(), "UNAVAIL".to_owned()));

        let gone = &pools[2];
        assert!(!gone.can_import);
        assert_eq!(
            gone.action.as_deref(),
            Some("The pool cannot be imported. Attach the missing devices and try again.")
        );
        assert_eq!(gone.devices, [("ata-QEMU_HARDDISK_QM00013-part1".to_owned(), "UNAVAIL".to_owned())]);
    }

    #[test]
    fn a_pool_this_machine_used_last_is_imported_without_force() {
        let pools = parse(include_str!("fixtures/zpool-import-same-host.txt")).unwrap();
        assert_eq!(pools.len(), 1);
        assert!(!pools[0].other_host && pools[0].can_import);
        assert_eq!(pools[0].status, None);
    }

    #[test]
    fn nothing_found_is_no_pool_and_odd_output_is_refused() {
        assert!(parse("").unwrap().is_empty());
        assert!(parse("     id: 12\n").is_err(), "an id before any pool");
        assert!(parse("   pool: tank\n  state: ONLINE\n").is_err(), "a pool without an id");
        assert!(parse("   pool: tank\n     id: twelve\n").is_err());
        assert!(is_group("raidz2-0") && is_group("draid2:4d:6c:1s-0") && is_group("logs"));
        assert!(!is_group("ata-QEMU_HARDDISK_QM00007-part1"));
    }
}
