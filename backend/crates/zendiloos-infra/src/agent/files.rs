use std::os::fd::OwnedFd;
use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    self as proto, AgentTransport, EntryKind as ProtoKind, ErrorCode, Request, Response, UserName,
};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_sharing::application::ports::{FileHost, FileHostError};
use zendiloos_sharing::domain::{Entry, EntryKind, FileLocation, FilePath, FileRoot, Listing};

use super::{WHAT, agent_error, unexpected};

/// [`FileHost`] through the files agent, which carries out each request
/// with the user's credentials.
pub struct AgentFiles {
    transport: Arc<dyn AgentTransport>,
}

impl AgentFiles {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    async fn call(
        &self,
        request: Request,
        context: &RequestContext,
    ) -> Result<(Response, Option<OwnedFd>), FileHostError> {
        let request_id = context.request_id.as_deref().and_then(|id| proto::RequestId::parse(id).ok());
        let answer =
            self.transport.call_with_file(request, request_id).await.map_err(|e| PortError::new(WHAT, e))?;
        match answer {
            (Response::Error { code, message }, _) => Err(match code {
                ErrorCode::NotFound => FileHostError::NotFound,
                ErrorCode::Denied => FileHostError::Denied,
                ErrorCode::Conflict => FileHostError::Exists,
                ErrorCode::InvalidRequest => FileHostError::Invalid(message),
                code => agent_error(code, &message).into(),
            }),
            answer => Ok(answer),
        }
    }

    async fn done(&self, request: Request, context: &RequestContext) -> Result<(), FileHostError> {
        match self.call(request, context).await? {
            (Response::Done, _) => Ok(()),
            (other, _) => Err(unexpected(other).into()),
        }
    }
}

fn invalid(what: &str) -> FileHostError {
    FileHostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn user(name: &str) -> Result<UserName, FileHostError> {
    UserName::parse(name).map_err(|_| invalid("user name"))
}

fn location(at: &FileLocation) -> Result<proto::FileLocation, FileHostError> {
    Ok(proto::FileLocation {
        volume: proto::VolumeNumber::new(at.volume).map_err(|_| invalid("volume number"))?,
        root: match &at.root {
            FileRoot::Share(share) => proto::FileRoot::Share(
                proto::ShareName::parse(share.as_str()).map_err(|_| invalid("share name"))?,
            ),
            FileRoot::Home => proto::FileRoot::Home,
        },
        path: proto::SharePath::parse(at.path.as_str()).map_err(|_| invalid("path"))?,
        snapshot: match &at.snapshot {
            Some(label) => Some(proto::SnapshotLabel::parse(label).map_err(|_| invalid("snapshot"))?),
            None => None,
        },
    })
}

fn file(descriptor: Option<OwnedFd>) -> Result<tokio::fs::File, FileHostError> {
    let descriptor = descriptor.ok_or_else(|| PortError::new(WHAT, "the answer carried no file"))?;
    Ok(tokio::fs::File::from_std(std::fs::File::from(descriptor)))
}

#[async_trait]
impl FileHost for AgentFiles {
    async fn list(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<Listing, FileHostError> {
        let request = Request::ListDirectory { user: user(user_name)?, location: location(at)? };
        match self.call(request, context).await? {
            (Response::Directory { entries, truncated, writable }, _) => Ok(Listing {
                entries: entries
                    .into_iter()
                    .map(|entry| Entry {
                        name: entry.name,
                        kind: match entry.kind {
                            ProtoKind::File => EntryKind::File,
                            ProtoKind::Directory => EntryKind::Directory,
                            ProtoKind::Symlink => EntryKind::Symlink,
                            ProtoKind::Other => EntryKind::Other,
                        },
                        size: entry.size,
                        modified: Timestamp::from_unix_seconds(entry.modified),
                        unreadable: entry.unreadable,
                    })
                    .collect(),
                truncated,
                writable,
            }),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    async fn open(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError> {
        let request = Request::OpenFile { user: user(user_name)?, location: location(at)? };
        match self.call(request, context).await? {
            (Response::File { size }, descriptor) => Ok((file(descriptor)?, size)),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    async fn create_upload(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, String), FileHostError> {
        let request = Request::CreateUpload { user: user(user_name)?, location: location(at)? };
        match self.call(request, context).await? {
            (Response::Upload { name }, descriptor) => Ok((file(descriptor)?, name)),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    async fn make_directory(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(), FileHostError> {
        self.done(Request::MakeDirectory { user: user(user_name)?, location: location(at)? }, context).await
    }

    async fn move_entry(
        &self,
        user_name: &str,
        from: &FileLocation,
        to: &FileLocation,
        replace: bool,
        context: &RequestContext,
    ) -> Result<(), FileHostError> {
        let request =
            Request::MoveEntry { user: user(user_name)?, from: location(from)?, to: location(to)?, replace };
        self.done(request, context).await
    }

    async fn copy_entry(
        &self,
        user_name: &str,
        from: &FileLocation,
        to: &FileLocation,
        context: &RequestContext,
    ) -> Result<FilePath, FileHostError> {
        let request = Request::CopyEntry { user: user(user_name)?, from: location(from)?, to: location(to)? };
        match self.call(request, context).await? {
            (Response::Copied { path }, _) => FilePath::parse(path.as_str()).map_err(|_| invalid("path")),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    async fn remove(
        &self,
        user_name: &str,
        at: &FileLocation,
        recycle: bool,
        context: &RequestContext,
    ) -> Result<(), FileHostError> {
        self.done(Request::RemoveEntry { user: user(user_name)?, location: location(at)?, recycle }, context)
            .await
    }

    async fn discard_upload(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(), FileHostError> {
        self.done(Request::DiscardUpload { user: user(user_name)?, location: location(at)? }, context).await
    }

    async fn archive(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<(tokio::fs::File, u64), FileHostError> {
        let request = Request::ArchiveFolder { user: user(user_name)?, location: location(at)? };
        match self.call(request, context).await? {
            (Response::Archive { size }, descriptor) => Ok((file(descriptor)?, size)),
            (other, _) => Err(unexpected(other).into()),
        }
    }

    async fn restore(
        &self,
        user_name: &str,
        at: &FileLocation,
        context: &RequestContext,
    ) -> Result<FilePath, FileHostError> {
        let request = Request::RestoreEntry { user: user(user_name)?, location: location(at)? };
        match self.call(request, context).await? {
            (Response::Restored { path }, _) => FilePath::parse(path.as_str()).map_err(|_| invalid("path")),
            (other, _) => Err(unexpected(other).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zendiloos_agent_proto::{PoolName, ShareName as ProtoShare, VolumeNumber};
    use zendiloos_sharing::domain::ShareName;
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[tokio::test]
    async fn files_travel_through_the_agent() {
        let root = tempfile::tempdir().unwrap();
        let agent = Arc::new(SimulatedAgent::new().with_files(root.path()));
        let share = Request::CreateShare {
            pool: PoolName::parse("tank").unwrap(),
            volume: VolumeNumber::new(1).unwrap(),
            name: ProtoShare::parse("docs").unwrap(),
        };
        agent.call(share, None).await.unwrap();
        let files = AgentFiles::new(agent);
        let context = RequestContext::default();
        let at = |path: &str| FileLocation {
            volume: 1,
            root: FileRoot::Share(ShareName::parse("docs").unwrap()),
            path: FilePath::parse(path).unwrap(),
            snapshot: None,
        };

        let (mut upload, name) = files.create_upload("anna", &at(""), &context).await.unwrap();
        upload.write_all(b"report").await.unwrap();
        upload.sync_all().await.unwrap();
        files.move_entry("anna", &at(&name), &at("report.txt"), false, &context).await.unwrap();
        let (mut download, size) = files.open("anna", &at("report.txt"), &context).await.unwrap();
        let mut text = String::new();
        download.read_to_string(&mut text).await.unwrap();
        assert_eq!((text.as_str(), size), ("report", 6));

        let copy = files.copy_entry("anna", &at("report.txt"), &at("report.txt"), &context).await.unwrap();
        assert_eq!(copy.as_str(), "report (2).txt");
        let listing = files.list("anna", &at(""), &context).await.unwrap();
        let mut names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["report (2).txt", "report.txt"]);
        assert!(listing.entries.iter().all(|e| !e.unreadable));
        let odd = std::ffi::OsStr::from_bytes(b"odd-\xFF.txt");
        std::fs::write(root.path().join("volume1/docs").join(odd), "x").unwrap();
        let listing = files.list("anna", &at(""), &context).await.unwrap();
        let odd = listing.entries.iter().find(|e| e.name == "odd-\u{FFFD}.txt").unwrap();
        assert!(odd.unreadable, "flagged on its way through the agent");
        std::fs::remove_file(
            root.path().join("volume1/docs").join(std::ffi::OsStr::from_bytes(b"odd-\xFF.txt")),
        )
        .unwrap();
        assert!(matches!(files.open("anna", &at("nope"), &context).await, Err(FileHostError::NotFound)));
        assert!(matches!(
            files.move_entry("anna", &at("report.txt"), &at("report (2).txt"), false, &context).await,
            Err(FileHostError::Exists)
        ));
        files.remove("anna", &at("report.txt"), true, &context).await.unwrap();
        assert!(root.path().join("volume1/docs/.recycle/anna/report.txt").exists());
        let back = files.restore("anna", &at(".recycle/anna/report.txt"), &context).await.unwrap();
        assert_eq!(back.as_str(), "report.txt");
        assert!(root.path().join("volume1/docs/report.txt").exists());

        let (mut archive, size) = files.archive("anna", &at(""), &context).await.unwrap();
        let mut bytes = Vec::new();
        archive.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes.len() as u64, size, "the whole archive came through the pipe");
        assert_eq!(&bytes[..5], b"docs/", "named after the share");
    }
}
