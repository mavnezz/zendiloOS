//! Backup tasks, simulated: rsync's folders as listings by place, a run
//! adding its version (or a mirror's copy) to its task's folder, and a
//! server that answers with one host key. Nothing is copied.

use std::collections::BTreeMap;

use zendiloos_agent_proto::{
    BackupEnd, BackupMode, BackupRoot, BackupTransport, ErrorCode, Request, Response,
};

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGv1zDdNJ6P8b8HFqUvNiOXIdk9JKLy6vJwDGP1vbTqL zendiloos-backup";
const HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

#[derive(Debug, Default)]
pub(crate) struct SimBackup {
    /// rsync and the SSH client installed (the plugin's run of apt).
    pub(crate) installed: bool,
    key: bool,
    /// Folders by place: their entries' names, folders marked `/`.
    folders: BTreeMap<String, Vec<String>>,
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error { code, message: message.to_owned() }
}

/// A place as a key of the listings.
fn place(end: &BackupEnd) -> String {
    match end {
        BackupEnd::Nas { root: BackupRoot::Share { location }, path } => {
            format!("/volume{}/{}/{}", location.volume.get(), location.share.as_str(), path.as_str())
        }
        BackupEnd::Nas { root: BackupRoot::Drive { drive }, path } => {
            format!("drive:{drive}/{}", path.as_str())
        }
        BackupEnd::Remote { server, path } => {
            let module = match &server.transport {
                BackupTransport::Daemon { module } => format!("::{module}"),
                BackupTransport::Ssh { .. } => String::new(),
            };
            format!("{}{module}:{}/{}", server.host, server.port, path.as_str().trim_start_matches('/'))
        }
    }
}

fn joined(base: &str, name: &str) -> String {
    format!("{}/{name}", base.trim_end_matches('/'))
}

/// `rsync --list-only` of the entries.
fn listing(entries: &[String]) -> String {
    let mut lines = vec!["drwxr-xr-x           4096 2026/11/11 03:00:00 .".to_owned()];
    for entry in entries {
        lines.push(match entry.strip_suffix('/') {
            Some(folder) => format!("drwxr-xr-x           4096 2026/11/11 03:00:00 {folder}"),
            None => format!("-rw-r--r--           1024 2026/11/11 03:00:00 {entry}"),
        });
    }
    lines.join("\n")
}

impl SimBackup {
    /// The answer to a request about backups; `None` for other requests.
    pub(crate) fn handle(&mut self, request: &Request) -> Option<Response> {
        let not_installed =
            || error(ErrorCode::Failed, "the system is not set up for this: rsync is not installed");
        Some(match request {
            Request::BackupFacts {} => Response::BackupFacts {
                rsync: self.installed,
                ssh: self.installed,
                public_key: self.key.then(|| KEY.to_owned()),
            },
            Request::MakeBackupKey {} if !self.installed => not_installed(),
            Request::MakeBackupKey {} => {
                self.key = true;
                Response::BackupKey { public_key: KEY.to_owned() }
            }
            Request::ScanBackupHostKeys { host, port } => {
                Response::BackupHostKeys { keyscan: format!("[{host}]:{port} {HOST_KEY}\n") }
            }
            Request::ListBackup { at, .. } => match self.folders.get(&place(at)) {
                Some(entries) => Response::BackupListing { listing: listing(entries), missing: false },
                None => Response::BackupListing { listing: String::new(), missing: true },
            },
            Request::RunBackup { .. } | Request::RestoreBackup { .. } if !self.installed => not_installed(),
            Request::RunBackup { run, .. } => {
                let folder = joined(&place(&run.target), run.folder.as_str());
                let entries = self.folders.entry(folder.clone()).or_default();
                if !entries.iter().any(|e| e == ".zendiloos-backup") {
                    entries.push(".zendiloos-backup".into());
                }
                let made = match run.mode {
                    BackupMode::Mirror { .. } => "current/".to_owned(),
                    BackupMode::Versioned => format!("{}/", run.stamp),
                };
                if !entries.contains(&made) {
                    entries.push(made.clone());
                }
                let listed = listing(entries);
                let inside = joined(&folder, made.trim_end_matches('/'));
                self.folders.entry(inside).or_insert_with(|| vec!["photos/".into(), "notes.txt".into()]);
                Response::BackupRan {
                    files: 2,
                    bytes: 2048,
                    partial: None,
                    transfers: vec![">f+++++++++ notes.txt".into(), "cd+++++++++ photos/".into()],
                    transfers_total: 2,
                    listing: listed,
                }
            }
            Request::StopBackup { .. } => Response::Done,
            Request::PruneBackup { at, names, .. } => {
                if let Some(entries) = self.folders.get_mut(&place(at)) {
                    entries.retain(|entry| {
                        !names.iter().any(|name| entry.trim_end_matches('/') == name.as_str())
                    });
                }
                Response::Done
            }
            Request::RestoreBackup { .. } => {
                Response::BackupRestored { files: 2, bytes: 2048, partial: None }
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{
        BackupFolder, BackupPruned, BackupRun, BackupStamp, BackupTaskId, ShareLocation, ShareName,
        SharePath, VolumeNumber,
    };

    use super::*;

    fn share(name: &str) -> BackupEnd {
        BackupEnd::Nas {
            root: BackupRoot::Share {
                location: ShareLocation {
                    volume: VolumeNumber::new(1).unwrap(),
                    share: ShareName::parse(name).unwrap(),
                },
            },
            path: SharePath::default(),
        }
    }

    #[test]
    fn a_run_leaves_a_version_that_lists_and_prunes() {
        let mut sim = SimBackup::default();
        let run = Request::RunBackup {
            run: BackupRun {
                task: BackupTaskId::new(1).unwrap(),
                source: share("photos"),
                target: share("backups"),
                folder: BackupFolder::parse("photos").unwrap(),
                mode: BackupMode::Versioned,
                stamp: BackupStamp::parse("2026-11-11_030000Z").unwrap(),
                excludes: Vec::new(),
                hidden: true,
                bwlimit_kib: 0,
            },
            password: None,
        };
        assert!(matches!(sim.handle(&run), Some(Response::Error { .. })), "not installed");
        sim.installed = true;
        let Some(Response::BackupRan { listing, .. }) = sim.handle(&run) else { panic!() };
        assert!(listing.contains("2026-11-11_030000Z"));
        let folder = BackupEnd::Nas {
            root: BackupRoot::Share {
                location: ShareLocation {
                    volume: VolumeNumber::new(1).unwrap(),
                    share: ShareName::parse("backups").unwrap(),
                },
            },
            path: SharePath::parse("photos").unwrap(),
        };
        let prune = Request::PruneBackup {
            task: BackupTaskId::new(1).unwrap(),
            at: folder.clone(),
            names: vec![BackupPruned::parse("2026-11-11_030000Z").unwrap()],
            password: None,
        };
        sim.handle(&prune);
        let Some(Response::BackupListing { listing, missing }) =
            sim.handle(&Request::ListBackup { at: folder, password: None })
        else {
            panic!()
        };
        assert!(!missing && !listing.contains("2026-11-11_030000Z"));
        assert!(matches!(sim.handle(&Request::MakeBackupKey {}), Some(Response::BackupKey { .. })));
        assert!(matches!(
            sim.handle(&Request::BackupFacts {}),
            Some(Response::BackupFacts { public_key: Some(_), .. })
        ));
        assert_eq!(sim.handle(&Request::Hello {}), None);
    }
}
