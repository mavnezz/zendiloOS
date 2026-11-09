//! [`VolumeArchiver`] through the system agent: volumes backed up into
//! shares and restored from them, over a transport that waits as long as
//! that takes. The agent's steps reach the job as they come.

use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, ArchiveDigest, BackupStem, ErrorCode, Request, Response, ShareLocation,
    ShareName, SharePath, VolumeName, VolumeNumber, WrittenFile,
};
use zendiloos_containers::application::ports::{
    EngineError, FoundArchive, ScannedArchives, SharePlace, Steps, VolumeArchiver,
};
use zendiloos_containers::domain::{BackupFile, WrittenBackup};
use zendiloos_kernel::{RequestContext, Timestamp};

use super::{call_raw_reporting, unexpected};

pub struct AgentVolumeArchiver {
    transport: Arc<dyn AgentTransport>,
}

impl AgentVolumeArchiver {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    async fn ask(
        &self,
        request: Request,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Response, EngineError> {
        match call_raw_reporting(self.transport.as_ref(), request, &ToSteps(steps), context).await? {
            Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
                Err(EngineError::Refused(message))
            }
            Response::Error { message, .. } => Err(EngineError::Failed(message)),
            response => Ok(response),
        }
    }
}

/// The agent's steps to the job's.
struct ToSteps<'a>(&'a dyn Steps);

impl proto::Report for ToSteps<'_> {
    fn step(&self, step: &str, percent: Option<u8>) {
        self.0.step(step, percent);
    }
}

fn refused(error: impl std::fmt::Display) -> EngineError {
    EngineError::Refused(error.to_string())
}

fn share(place: &SharePlace) -> Result<ShareLocation, EngineError> {
    Ok(ShareLocation {
        volume: VolumeNumber::new(place.volume).map_err(refused)?,
        share: ShareName::parse(&place.share).map_err(refused)?,
    })
}

fn path(text: &str) -> Result<SharePath, EngineError> {
    SharePath::parse(text).map_err(refused)
}

fn file(written: WrittenFile) -> BackupFile {
    BackupFile {
        path: written.path.as_str().to_owned(),
        bytes: written.bytes,
        device: written.device,
        inode: written.inode,
        modified: Some(written.modified),
    }
}

#[async_trait]
impl VolumeArchiver for AgentVolumeArchiver {
    async fn back_up(
        &self,
        volume: &str,
        place: &SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<WrittenBackup, EngineError> {
        let request = Request::BackUpVolume {
            volume: VolumeName::parse(volume).map_err(refused)?,
            share: share(place)?,
            folder: path(folder)?,
            name: BackupStem::parse(stem).map_err(refused)?,
        };
        match self.ask(request, steps, context).await? {
            Response::VolumeBackedUp { archive, manifest, sha256, data_bytes, entries, manifest_json } => {
                Ok(WrittenBackup {
                    archive: file(archive),
                    manifest: file(manifest),
                    sha256: sha256.as_str().to_owned(),
                    data_bytes,
                    entries: Some(entries),
                    manifest_json,
                })
            }
            other => Err(unexpected(other).into()),
        }
    }

    async fn restore(
        &self,
        volume: &str,
        place: &SharePlace,
        written: &WrittenBackup,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Option<String>, EngineError> {
        let request = Request::RestoreVolume {
            volume: VolumeName::parse(volume).map_err(refused)?,
            share: share(place)?,
            archive: path(&written.archive.path)?,
            sha256: ArchiveDigest::parse(&written.sha256).map_err(refused)?,
            data_bytes: written.data_bytes,
            entries: written.entries,
        };
        match self.ask(request, steps, context).await? {
            Response::VolumeRestored { left_over } => Ok(left_over),
            other => Err(unexpected(other).into()),
        }
    }

    async fn remove(
        &self,
        place: &SharePlace,
        file: &BackupFile,
        context: &RequestContext,
    ) -> Result<bool, EngineError> {
        // Not by its device: a ZFS dataset gets another one as it is
        // mounted again.
        let request = Request::RemoveVolumeBackup {
            share: share(place)?,
            path: path(&file.path)?,
            inode: file.inode,
            bytes: file.bytes,
            modified: file.modified,
        };
        match self.ask(request, &zendiloos_containers::application::ports::NoSteps, context).await? {
            Response::BackupRemoved { removed } => Ok(removed),
            other => Err(unexpected(other).into()),
        }
    }

    async fn end_work(&self, volume: &str, context: &RequestContext) -> Result<bool, EngineError> {
        let request = Request::EndVolumeWork { volume: VolumeName::parse(volume).map_err(refused)? };
        match self.ask(request, &zendiloos_containers::application::ports::NoSteps, context).await? {
            Response::VolumeWorkEnded { stopped } => Ok(stopped),
            other => Err(unexpected(other).into()),
        }
    }

    async fn scan(
        &self,
        place: &SharePlace,
        folder: &str,
        known: &[String],
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<ScannedArchives, EngineError> {
        let request = Request::ScanVolumeBackups {
            share: share(place)?,
            folder: path(folder)?,
            known: known.iter().map(|known| path(known)).collect::<Result<_, _>>()?,
        };
        match self.ask(request, steps, context).await? {
            Response::VolumeBackupsScanned { found, refused } => Ok(ScannedArchives {
                found: found
                    .into_iter()
                    .map(|found| FoundArchive {
                        volume: found.volume.as_str().to_owned(),
                        backed_up_at: Timestamp::from_unix_seconds(found.backed_up_at),
                        written: WrittenBackup {
                            archive: file(found.archive),
                            manifest: file(found.manifest),
                            sha256: found.sha256.as_str().to_owned(),
                            data_bytes: found.data_bytes,
                            entries: found.entries,
                            manifest_json: found.manifest_json,
                        },
                    })
                    .collect(),
                refused: refused
                    .into_iter()
                    .map(|file| (file.path.as_str().to_owned(), file.reason))
                    .collect(),
            }),
            other => Err(unexpected(other).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zendiloos_sim::SimulatedAgent;

    use super::*;
    use crate::agent::scripted::Scripted;

    #[derive(Default)]
    struct Heard(Mutex<Vec<(String, Option<u8>)>>);

    impl Steps for Heard {
        fn step(&self, step: &str, percent: Option<u8>) {
            self.0.lock().unwrap().push((step.to_owned(), percent));
        }
    }

    fn place() -> SharePlace {
        SharePlace { volume: 1, share: "backups".into() }
    }

    /// A backup recorded as written to `archive` with `sha256`.
    fn written(archive: &str, sha256: &str) -> WrittenBackup {
        let file =
            |path: &str| BackupFile { path: path.into(), bytes: 1, device: 1, inode: 1, modified: None };
        WrittenBackup {
            archive: file(archive),
            manifest: file("m.json"),
            sha256: sha256.into(),
            data_bytes: 1,
            entries: None,
            manifest_json: String::new(),
        }
    }

    #[tokio::test]
    async fn volumes_are_backed_up_restored_and_their_files_removed_through_the_agent() {
        let archiver = AgentVolumeArchiver::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let heard = Heard::default();
        let written = archiver
            .back_up("stack_cache", &place(), "docker", "stack_cache-20261108-030000", &heard, &context)
            .await
            .unwrap();
        assert_eq!(written.archive.path, "docker/stack_cache/stack_cache-20261108-030000.tar.zst");
        assert_eq!(written.manifest.path, "docker/stack_cache/stack_cache-20261108-030000.json");
        assert_eq!(written.sha256.len(), 64);
        assert!(written.manifest_json.contains("stack_cache"));
        let restored = archiver.restore("old_data", &place(), &written, &heard, &context).await;
        assert!(restored.is_ok(), "{restored:?}");
        let busy = archiver.restore("stack_cache", &place(), &written, &heard, &context).await;
        assert!(matches!(busy, Err(EngineError::Refused(_))), "a running container uses it: {busy:?}");
        // Without shares on disk, the simulated agent finds none.
        let known = [written.archive.path.clone()];
        let scanned = archiver.scan(&place(), "docker", &known, &heard, &context).await.unwrap();
        assert_eq!(scanned, ScannedArchives::default());
        assert!(archiver.remove(&place(), &written.archive, &context).await.unwrap());
        assert!(!archiver.end_work("stack_cache", &context).await.unwrap(), "nothing runs");
        assert!(matches!(archiver.end_work("-x", &context).await, Err(EngineError::Refused(_))));
    }

    #[tokio::test]
    async fn what_the_agent_could_misread_is_refused_before_it_is_sent() {
        let agent = Arc::new(Scripted::default());
        let archiver = AgentVolumeArchiver::new(agent.clone());
        let context = RequestContext::default();
        let heard = Heard::default();
        let refused =
            |result: Result<(), EngineError>| assert!(matches!(result, Err(EngineError::Refused(_))));
        let backup = |volume: &str, place: SharePlace, folder: &str, stem: &str| {
            let (volume, folder, stem) = (volume.to_owned(), folder.to_owned(), stem.to_owned());
            let (archiver, heard, context) = (&archiver, &heard, &context);
            async move { archiver.back_up(&volume, &place, &folder, &stem, heard, context).await.map(|_| ()) }
        };
        refused(backup("-v", place(), "", "x1").await);
        refused(backup("db", SharePlace { volume: 0, share: "b".into() }, "", "x1").await);
        refused(backup("db", SharePlace { volume: 1, share: "Not A Share".into() }, "", "x1").await);
        refused(backup("db", place(), "../up", "x1").await);
        refused(backup("db", place(), "", "a/b").await);
        refused(
            archiver
                .restore("db", &place(), &written("a.tar.zst", "short"), &heard, &context)
                .await
                .map(drop),
        );
        let odd = BackupFile { path: "../x".into(), bytes: 0, device: 0, inode: 0, modified: None };
        assert!(matches!(archiver.remove(&place(), &odd, &context).await, Err(EngineError::Refused(_))));
        let up = archiver.scan(&place(), "../up", &[], &heard, &context).await;
        assert!(matches!(up, Err(EngineError::Refused(_))), "{up:?}");
        let known = archiver.scan(&place(), "old", &["/etc/x".into()], &heard, &context).await;
        assert!(matches!(known, Err(EngineError::Refused(_))), "{known:?}");
        assert!(agent.asked().is_empty(), "nothing odd was sent");
    }

    #[tokio::test]
    async fn the_agents_answers_are_told_apart() {
        let context = RequestContext::default();
        let heard = Heard::default();
        let answering =
            |response: Response| AgentVolumeArchiver::new(Arc::new(Scripted::answering([response])));
        let digest = "ab".repeat(32);
        let refused =
            answering(Response::Error { code: ErrorCode::Forbidden, message: "SHA-256 differs".into() });
        let result = refused.restore("db", &place(), &written("a.tar.zst", &digest), &heard, &context).await;
        assert!(matches!(result, Err(EngineError::Refused(_))));
        let failed = answering(Response::Error { code: ErrorCode::Failed, message: "no space".into() });
        let result = failed.back_up("db", &place(), "", "db-1", &heard, &context).await;
        assert!(matches!(result, Err(EngineError::Failed(_))));
        let odd = answering(Response::Done);
        assert!(matches!(
            odd.back_up("db", &place(), "", "db-1", &heard, &context).await,
            Err(EngineError::Port(_))
        ));
        let odd = answering(Response::BackupRemoved { removed: true });
        assert!(odd.restore("db", &place(), &written("a", &digest), &heard, &context).await.is_err());
        let odd = answering(Response::Done);
        let file = BackupFile { path: "a".into(), bytes: 0, device: 0, inode: 0, modified: None };
        assert!(odd.remove(&place(), &file, &context).await.is_err());
        let odd = answering(Response::Done);
        assert!(odd.end_work("db", &context).await.is_err());
        let ended = answering(Response::VolumeWorkEnded { stopped: true });
        assert!(ended.end_work("db", &context).await.unwrap());
        let odd = answering(Response::Done);
        assert!(odd.scan(&place(), "", &[], &heard, &context).await.is_err());
        let at = |path: &str, inode: u64| WrittenFile {
            path: SharePath::parse(path).unwrap(),
            bytes: 10,
            device: 2,
            inode,
            modified: 1_762_570_800_000_000_000,
        };
        let scanned = answering(Response::VolumeBackupsScanned {
            found: vec![proto::FoundBackup {
                volume: VolumeName::parse("db").unwrap(),
                archive: at("old/db/db-20261108-030000.tar.zst", 5),
                manifest: at("old/db/db-20261108-030000.json", 6),
                sha256: ArchiveDigest::parse(&digest).unwrap(),
                data_bytes: 4096,
                entries: None,
                backed_up_at: 1_762_570_800,
                manifest_json: "{}".into(),
            }],
            refused: vec![proto::RefusedBackupFile {
                path: SharePath::parse("old/db-1.tar.zst").unwrap(),
                reason: "the archive: writable by others".into(),
            }],
        });
        let scanned = scanned.scan(&place(), "old", &[], &heard, &context).await.unwrap();
        let found = &scanned.found[0];
        assert_eq!(
            (found.volume.as_str(), found.backed_up_at.unix_seconds(), found.written.entries),
            ("db", 1_762_570_800, None)
        );
        assert_eq!(found.written.archive.path, "old/db/db-20261108-030000.tar.zst");
        assert_eq!(
            (found.written.manifest.inode, found.written.manifest.modified),
            (6, Some(1_762_570_800_000_000_000))
        );
        assert_eq!((found.written.sha256.as_str(), found.written.data_bytes), (digest.as_str(), 4096));
        assert_eq!(
            scanned.refused,
            [("old/db-1.tar.zst".to_owned(), "the archive: writable by others".to_owned())]
        );
    }
}
