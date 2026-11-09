//! Docker's volumes for the simulation: two of the project `stack`, one
//! that nothing uses, and those made while it runs. Backups are written
//! into the shares' scratch folders when there are some (a small file
//! standing in for the archive, with its real SHA-256 and a manifest),
//! restores check that SHA-256, and nothing is extracted anywhere.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    ErrorCode, FoundBackup, RefusedBackupFile, Request, Response, ShareLocation, SharePath, WrittenFile,
};

#[derive(Debug, Clone)]
pub(crate) struct SimVolume {
    name: String,
    labels: BTreeMap<String, String>,
    created: String,
}

pub(crate) fn initial() -> Vec<SimVolume> {
    let volume = |name: &str, project: Option<&str>| SimVolume {
        name: name.to_owned(),
        labels: project
            .map(|p| BTreeMap::from([("com.docker.compose.project".to_owned(), p.to_owned())]))
            .unwrap_or_default(),
        created: "2026-10-20T10:00:00Z".to_owned(),
    };
    vec![volume("stack_cache", Some("stack")), volume("stack_db", Some("stack")), volume("old_data", None)]
}

fn failed(message: &str) -> Response {
    Response::Error { code: ErrorCode::Failed, message: message.to_owned() }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The folder of a share in the scratch folder, if there is one.
fn share_folder(files: Option<&Path>, share: &ShareLocation) -> Option<PathBuf> {
    files.map(|root| root.join(format!("volume{}", share.volume.get())).join(share.share.as_str()))
}

fn written(path: &SharePath, file: &Path) -> std::io::Result<WrittenFile> {
    let meta = std::fs::metadata(file)?;
    let modified = meta.mtime().saturating_mul(1_000_000_000).saturating_add(meta.mtime_nsec());
    Ok(WrittenFile { path: path.clone(), bytes: meta.len(), device: meta.dev(), inode: meta.ino(), modified })
}

/// Answers the requests about volumes; `used` tells which volumes
/// containers use, and whether one of those runs.
pub(crate) fn handle(
    volumes: &mut Vec<SimVolume>,
    request: &Request,
    used: &dyn Fn(&str) -> Option<bool>,
    files: Option<&Path>,
    now: u64,
) -> Option<Response> {
    let output = |text: String| Some(Response::DockerOutput { text });
    match request {
        Request::DockerVolumes {} => {
            let listed: Vec<Value> = volumes
                .iter()
                .map(|v| {
                    json!({"Name": v.name, "Driver": "local", "Scope": "local", "Options": null,
                           "Mountpoint": format!("/volume1/@docker/volumes/{}/_data", v.name),
                           "Labels": if v.labels.is_empty() { Value::Null } else { json!(v.labels) },
                           "CreatedAt": v.created})
                })
                .collect();
            output(Value::from(listed).to_string())
        }
        Request::DockerVolumeSizes {} => Some(Response::VolumeSizes {
            sizes: volumes
                .iter()
                .enumerate()
                .map(|(n, v)| zendiloos_agent_proto::VolumeSize {
                    name: v.name.clone(),
                    bytes: 37 * 1024 * 1024 * (n as u64 + 1),
                    complete: true,
                })
                .collect(),
        }),
        Request::RemoveVolume { name } => {
            if used(name.as_str()).is_some() {
                return Some(failed(&format!(
                    "Docker: Error response from daemon: remove {}: volume is in use",
                    name.as_str()
                )));
            }
            let before = volumes.len();
            volumes.retain(|v| v.name != name.as_str());
            if volumes.len() == before {
                return Some(failed("Docker: Error response from daemon: get volume: no such volume"));
            }
            Some(Response::Done)
        }
        Request::PruneVolumes {} => {
            let gone: Vec<String> =
                volumes.iter().filter(|v| used(&v.name).is_none()).map(|v| v.name.clone()).collect();
            volumes.retain(|v| !gone.contains(&v.name));
            output(format!(
                "Deleted Volumes:\n{}\n\nTotal reclaimed space: {}MB\n",
                gone.join("\n"),
                gone.len() * 37
            ))
        }
        Request::CreateVolume { name, labels } => {
            if !volumes.iter().any(|v| v.name == name.as_str()) {
                volumes.push(SimVolume {
                    name: name.as_str().to_owned(),
                    labels: labels.as_map().clone(),
                    created: jiff::Timestamp::from_second(i64::try_from(now).unwrap_or(0))
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                });
            }
            Some(Response::Done)
        }
        Request::BackUpVolume { volume, share, folder, name } => {
            let Some(found) = volumes.iter().find(|v| v.name == volume.as_str()) else {
                return Some(failed("Docker: no such volume"));
            };
            let content = format!("simulated backup of the volume {} at {now}\n", found.name).into_bytes();
            let sha256 = digest(&content);
            let folder = folder.join(volume.as_str()).ok()?;
            let archive_path = folder.join(&format!("{}.tar.zst", name.as_str())).ok()?;
            let manifest_path = folder.join(&format!("{}.json", name.as_str())).ok()?;
            let manifest_json = serde_json::to_string_pretty(&json!({
                "format": 1, "volume": found.name, "driver": "local", "labels": found.labels,
                "size_bytes": 37 * 1024 * 1024, "created": found.created, "backed_up_at": now,
                "zendiloos_version": env!("CARGO_PKG_VERSION"), "archive": archive_path.file_name(), "sha256": sha256,
            }))
            .unwrap_or_default();
            let (archive, manifest) = match share_folder(files, share) {
                Some(root) => {
                    let written = (|| {
                        std::fs::create_dir_all(root.join(folder.as_str()))?;
                        std::fs::write(root.join(archive_path.as_str()), &content)?;
                        std::fs::write(root.join(manifest_path.as_str()), &manifest_json)?;
                        Ok::<_, std::io::Error>((
                            written(&archive_path, &root.join(archive_path.as_str()))?,
                            written(&manifest_path, &root.join(manifest_path.as_str()))?,
                        ))
                    })();
                    match written {
                        Ok(files) => files,
                        Err(error) => return Some(failed(&format!("cannot write the backup: {error}"))),
                    }
                }
                None => (
                    WrittenFile {
                        path: archive_path,
                        bytes: content.len() as u64,
                        device: 1,
                        inode: now,
                        modified: 0,
                    },
                    WrittenFile {
                        path: manifest_path,
                        bytes: manifest_json.len() as u64,
                        device: 1,
                        inode: now + 1,
                        modified: 0,
                    },
                ),
            };
            Some(Response::VolumeBackedUp {
                archive,
                manifest,
                sha256: zendiloos_agent_proto::ArchiveDigest::parse(&sha256).ok()?,
                data_bytes: 37 * 1024 * 1024,
                entries: 120,
                manifest_json,
            })
        }
        Request::RestoreVolume { volume, share, archive, sha256, .. } => {
            if !volumes.iter().any(|v| v.name == volume.as_str()) {
                return Some(failed("Docker: no such volume"));
            }
            if used(volume.as_str()) == Some(true) {
                return Some(Response::Error {
                    code: ErrorCode::Conflict,
                    message: format!(
                        "in use: running containers use the volume {}: stop them first",
                        volume.as_str()
                    ),
                });
            }
            if let Some(root) = share_folder(files, share) {
                let Ok(bytes) = std::fs::read(root.join(archive.as_str())) else {
                    return Some(Response::Error {
                        code: ErrorCode::Forbidden,
                        message: format!("refused: {} is not there any more", archive.as_str()),
                    });
                };
                if digest(&bytes) != sha256.as_str() {
                    return Some(Response::Error {
                        code: ErrorCode::Forbidden,
                        message: format!(
                            "refused: {} is not the archive that was backed up",
                            archive.as_str()
                        ),
                    });
                }
            }
            Some(Response::VolumeRestored { left_over: None })
        }
        Request::RemoveVolumeBackup { share, path, inode, bytes, .. } => {
            let removed = match share_folder(files, share) {
                Some(root) => {
                    let file = root.join(path.as_str());
                    let same = std::fs::symlink_metadata(&file)
                        .is_ok_and(|m| m.is_file() && m.ino() == *inode && m.len() == *bytes);
                    same && std::fs::remove_file(&file).is_ok()
                }
                None => true,
            };
            Some(Response::BackupRemoved { removed })
        }
        // Nothing of a volume's work outlives its request here.
        Request::EndVolumeWork { .. } => Some(Response::VolumeWorkEnded { stopped: false }),
        Request::ScanVolumeBackups { share, folder, known } => {
            let (found, refused) = match share_folder(files, share) {
                Some(root) => scan(&root, folder, known),
                None => (Vec::new(), Vec::new()),
            };
            Some(Response::VolumeBackupsScanned { found, refused })
        }
        _ => None,
    }
}

/// The simulated backups in `folder` of a share and the folders in it: a
/// manifest beside each archive, naming its SHA-256.
fn scan(root: &Path, folder: &SharePath, known: &[SharePath]) -> (Vec<FoundBackup>, Vec<RefusedBackupFile>) {
    let (mut found, mut refused) = (Vec::new(), Vec::new());
    let mut folders = vec![folder.clone()];
    if let Ok(entries) = std::fs::read_dir(root.join(folder.as_str())) {
        for entry in entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            if let Ok(inner) = folder.join(&entry.file_name().to_string_lossy()) {
                folders.push(inner);
            }
        }
    }
    for at in folders {
        let Ok(entries) = std::fs::read_dir(root.join(at.as_str())) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".tar.zst") else { continue };
            let Ok(archive) = at.join(&name) else { continue };
            if known.contains(&archive) {
                continue;
            }
            let Ok(manifest) = at.join(&format!("{stem}.json")) else { continue };
            let read = std::fs::read_to_string(root.join(manifest.as_str())).ok();
            let value: Value =
                read.as_deref().and_then(|text| serde_json::from_str(text).ok()).unwrap_or_default();
            let bytes = std::fs::read(root.join(archive.as_str())).unwrap_or_default();
            let volume =
                value["volume"].as_str().and_then(|v| zendiloos_agent_proto::VolumeName::parse(v).ok());
            let sha256 = zendiloos_agent_proto::ArchiveDigest::parse(&digest(&bytes)).ok();
            let files = (
                written(&archive, &root.join(archive.as_str())),
                written(&manifest, &root.join(manifest.as_str())),
            );
            match (volume, sha256, files, read) {
                (Some(volume), Some(sha256), (Ok(archive), Ok(manifest)), Some(manifest_json))
                    if value["sha256"].as_str() == Some(sha256.as_str()) =>
                {
                    found.push(FoundBackup {
                        volume,
                        archive,
                        manifest,
                        sha256,
                        data_bytes: value["size_bytes"].as_u64().unwrap_or(0),
                        entries: value["entries"].as_u64(),
                        backed_up_at: value["backed_up_at"].as_i64().unwrap_or(0),
                        manifest_json,
                    });
                }
                _ => {
                    refused.push(RefusedBackupFile { path: archive, reason: "no manifest beside it".into() })
                }
            }
        }
    }
    (found, refused)
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{
        AgentTransport, ArchiveDigest, BackupStem, ContainerAction, ContainerId, PoolName, ShareName,
        VolumeLabels, VolumeName, VolumeNumber,
    };

    use super::*;
    use crate::SimulatedAgent;

    fn name(text: &str) -> VolumeName {
        VolumeName::parse(text).unwrap()
    }

    #[tokio::test]
    async fn volumes_are_listed_made_removed_and_backed_up_into_the_shares() {
        let root = tempfile::tempdir().unwrap();
        let agent = SimulatedAgent::new().with_files(root.path());
        let (volume, backups) = (VolumeNumber::new(1).unwrap(), ShareName::parse("backups").unwrap());
        let pool = PoolName::parse("tank").unwrap();
        agent.call(Request::CreateShare { pool, volume, name: backups.clone() }, None).await.unwrap();
        let call = |request: Request| agent.call(request, None);

        let Response::DockerOutput { text } = call(Request::DockerVolumes {}).await.unwrap() else {
            panic!()
        };
        assert!(text.contains("stack_cache") && text.contains("old_data"), "{text}");
        let Response::VolumeSizes { sizes } = call(Request::DockerVolumeSizes {}).await.unwrap() else {
            panic!()
        };
        assert_eq!(sizes.len(), 3);
        let in_use = call(Request::RemoveVolume { name: name("stack_cache") }).await.unwrap();
        assert!(
            matches!(&in_use, Response::Error { message, .. } if message.contains("in use")),
            "{in_use:?}"
        );
        let fresh = Request::CreateVolume { name: name("fresh"), labels: VolumeLabels::default() };
        assert_eq!(call(fresh).await.unwrap(), Response::Done);
        let Response::DockerOutput { text } = call(Request::PruneVolumes {}).await.unwrap() else { panic!() };
        assert!(
            text.contains("fresh") && text.contains("old_data") && !text.contains("stack_cache"),
            "{text}"
        );
        let gone = call(Request::RemoveVolume { name: name("fresh") }).await.unwrap();
        assert!(matches!(gone, Response::Error { .. }));

        let share = ShareLocation { volume, share: backups };
        let backup = Request::BackUpVolume {
            volume: name("stack_cache"),
            share: share.clone(),
            folder: SharePath::parse("docker").unwrap(),
            name: BackupStem::parse("stack_cache-20261108-030000").unwrap(),
        };
        let Response::VolumeBackedUp { archive, manifest, sha256, manifest_json, .. } =
            call(backup).await.unwrap()
        else {
            panic!()
        };
        let folder = root.path().join("volume1/backups/docker/stack_cache");
        assert!(folder.join("stack_cache-20261108-030000.tar.zst").is_file());
        assert!(manifest_json.contains(sha256.as_str()));
        let restore = |sha256: ArchiveDigest| Request::RestoreVolume {
            volume: name("stack_cache"),
            share: share.clone(),
            archive: archive.path.clone(),
            sha256,
            data_bytes: 37 << 20,
            entries: Some(120),
        };
        // Not while its container runs; then only the archive recorded.
        let busy = call(restore(sha256.clone())).await.unwrap();
        assert!(matches!(busy, Response::Error { code: ErrorCode::Conflict, .. }), "{busy:?}");
        let web = ContainerId::parse("stack-web-1").unwrap();
        call(Request::ControlContainer { container: web, action: ContainerAction::Stop }).await.unwrap();
        assert_eq!(
            call(restore(sha256.clone())).await.unwrap(),
            Response::VolumeRestored { left_over: None }
        );
        let wrong = call(restore(ArchiveDigest::parse(&"0".repeat(64)).unwrap())).await.unwrap();
        assert!(matches!(wrong, Response::Error { code: ErrorCode::Forbidden, .. }), "{wrong:?}");

        let remove = |file: &WrittenFile| Request::RemoveVolumeBackup {
            share: share.clone(),
            path: file.path.clone(),
            inode: file.inode,
            bytes: file.bytes,
            modified: Some(file.modified),
        };
        let scan = Request::ScanVolumeBackups {
            share: share.clone(),
            folder: SharePath::parse("docker").unwrap(),
            known: vec![],
        };
        let Response::VolumeBackupsScanned { found, refused } = call(scan).await.unwrap() else { panic!() };
        assert_eq!((found.len(), refused.len()), (1, 0));
        assert_eq!(found[0].archive.path, archive.path);
        assert_eq!(call(remove(&manifest)).await.unwrap(), Response::BackupRemoved { removed: true });
        assert_eq!(call(remove(&manifest)).await.unwrap(), Response::BackupRemoved { removed: false });
        assert!(!folder.join("stack_cache-20261108-030000.json").exists());
        let unknown = Request::BackUpVolume {
            volume: name("nothing"),
            share,
            folder: SharePath::root(),
            name: BackupStem::parse("x1").unwrap(),
        };
        assert!(matches!(call(unknown).await.unwrap(), Response::Error { .. }));
    }

    #[tokio::test]
    async fn without_shares_on_disk_backups_are_answered_all_the_same() {
        let agent = SimulatedAgent::new();
        let share =
            ShareLocation { volume: VolumeNumber::new(1).unwrap(), share: ShareName::parse("b1").unwrap() };
        let backup = Request::BackUpVolume {
            volume: name("old_data"),
            share: share.clone(),
            folder: SharePath::root(),
            name: BackupStem::parse("old_data-20261108-030000").unwrap(),
        };
        let Response::VolumeBackedUp { archive, sha256, .. } = agent.call(backup, None).await.unwrap() else {
            panic!()
        };
        assert_eq!(archive.path.as_str(), "old_data/old_data-20261108-030000.tar.zst");
        let restore = Request::RestoreVolume {
            volume: name("old_data"),
            share: share.clone(),
            archive: archive.path.clone(),
            sha256,
            data_bytes: 37 << 20,
            entries: None,
        };
        assert_eq!(agent.call(restore, None).await.unwrap(), Response::VolumeRestored { left_over: None });
        let remove =
            Request::RemoveVolumeBackup { share, path: archive.path, inode: 1, bytes: 1, modified: None };
        assert_eq!(agent.call(remove, None).await.unwrap(), Response::BackupRemoved { removed: true });
    }
}
