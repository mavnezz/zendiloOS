//! The files agent's work inside shares. Each request runs on a thread of
//! its own that first takes on the credentials of a managed user, so the
//! file system checks permissions and ACLs exactly as for that user's SMB
//! and NFS access. On Linux, credentials belong to a thread when set with
//! the raw system calls (as rustix does; glibc's wrappers would change the
//! whole process), and they end with the thread.

use std::io;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

use rustix::process::{Gid, Uid, getegid, geteuid, getgid, getuid};
use rustix::thread::{capabilities, set_thread_groups, set_thread_res_gid, set_thread_res_uid};
use zendiloos_agent_proto::{FileLocation, Request, Response, UserName};
use zendiloos_fileops::{FileError, ShareRoot};

use crate::accounts::{AccountFiles, ID_MAX, ID_MIN, USERS_GROUP};
use crate::operations::OperationError;
use crate::passwd;

#[derive(Debug)]
pub(crate) struct Files {
    accounts: AccountFiles,
    volumes: PathBuf,
}

/// Credentials of a managed user.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    uid: u32,
    gid: u32,
    /// Supplementary groups, the primary one included.
    groups: Vec<u32>,
}

fn read(path: &std::path::Path) -> Result<String, OperationError> {
    std::fs::read_to_string(path).map_err(|e| OperationError::Read(path.to_owned(), e))
}

/// Takes on `identity` for the rest of this thread's life.
fn become_user(identity: &Identity) -> io::Result<()> {
    let groups: Vec<Gid> = identity.groups.iter().map(|&gid| Gid::from_raw(gid)).collect();
    let (uid, gid) = (Uid::from_raw(identity.uid), Gid::from_raw(identity.gid));
    set_thread_groups(&groups)?;
    set_thread_res_gid(gid, gid, gid)?;
    // Last: without UID 0 the thread loses the right to change the others,
    // and all its capabilities.
    set_thread_res_uid(uid, uid, uid)?;
    let capabilities = capabilities(None)?;
    let switched = getuid() == uid && geteuid() == uid && getgid() == gid && getegid() == gid;
    if !switched || !capabilities.effective.is_empty() || !capabilities.permitted.is_empty() {
        return Err(io::Error::other("the thread kept privileges"));
    }
    Ok(())
}

/// Runs `work` on a new thread with `identity`'s credentials.
async fn as_user<T: Send + 'static>(
    identity: Identity,
    work: impl FnOnce() -> Result<T, FileError> + Send + 'static,
) -> Result<T, OperationError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("files-as-user".to_owned())
        .spawn(move || {
            let result = match become_user(&identity) {
                Ok(()) => work().map_err(OperationError::File),
                Err(error) => Err(OperationError::Credentials(error)),
            };
            let _ = sender.send(result);
        })
        .map_err(OperationError::Credentials)?;
    receiver.await.map_err(|_| OperationError::Credentials(io::Error::other("the thread ended early")))?
}

impl Files {
    pub(crate) fn new(accounts: AccountFiles, volumes: PathBuf) -> Self {
        Self { accounts, volumes }
    }

    /// Only users zendiloos manages: never root, a system account or the
    /// owner's own user.
    fn identity(&self, user: &UserName) -> Result<Identity, OperationError> {
        let not_managed = || OperationError::NotManaged(user.as_str().to_owned());
        let (passwd, group) = (read(&self.accounts.passwd)?, read(&self.accounts.group)?);
        let (uid, gid) = passwd::lookup(&passwd, user.as_str()).ok_or_else(not_managed)?;
        if !(ID_MIN..=ID_MAX).contains(&uid) || passwd::group_id(&group, USERS_GROUP) != Some(gid) {
            return Err(not_managed());
        }
        let mut groups = passwd::memberships(&group, user.as_str());
        groups.push(gid);
        groups.sort_unstable();
        groups.dedup();
        Ok(Identity { uid, gid, groups })
    }

    /// Opened as root, before the switch: refused unless the share's own
    /// dataset is mounted.
    fn share(&self, location: &FileLocation) -> Result<ShareRoot, OperationError> {
        ShareRoot::open(&self.volumes, location.volume, &location.share, true).map_err(OperationError::File)
    }

    pub(crate) async fn execute(
        &self,
        request: &Request,
    ) -> Result<(Response, Option<OwnedFd>), OperationError> {
        match request {
            Request::ListDirectory { user, location } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                let listing = as_user(identity, move || share.list(&path)).await?;
                let response = Response::Directory {
                    entries: listing.entries,
                    truncated: listing.truncated,
                    writable: listing.writable,
                };
                Ok((response, None))
            }
            Request::OpenFile { user, location } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                let (file, size) = as_user(identity, move || share.open_file(&path)).await?;
                Ok((Response::File { size }, Some(file)))
            }
            Request::CreateUpload { user, location } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                let (file, name) = as_user(identity, move || share.create_upload(&path)).await?;
                Ok((Response::Upload { name }, Some(file)))
            }
            Request::MakeDirectory { user, location } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                as_user(identity, move || share.make_directory(&path)).await?;
                Ok((Response::Done, None))
            }
            Request::MoveEntry { user, from, to, replace } => {
                let identity = self.identity(user)?;
                let (source, target) = (self.share(from)?, self.share(to)?);
                let (from, to, replace) = (from.path.clone(), to.path.clone(), *replace);
                as_user(identity, move || source.move_entry(&from, &target, &to, replace)).await?;
                Ok((Response::Done, None))
            }
            Request::CopyEntry { user, from, to } => {
                let identity = self.identity(user)?;
                let (source, target) = (self.share(from)?, self.share(to)?);
                let (from, to) = (from.path.clone(), to.path.clone());
                let path = as_user(identity, move || source.copy_entry(&from, &target, &to)).await?;
                Ok((Response::Copied { path }, None))
            }
            Request::RemoveEntry { user, location, recycle } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                let bin_owner = recycle.then(|| user.as_str().to_owned());
                as_user(identity, move || share.remove(&path, bin_owner.as_deref())).await?;
                Ok((Response::Done, None))
            }
            Request::RestoreEntry { user, location } => {
                let (identity, share, path) =
                    (self.identity(user)?, self.share(location)?, location.path.clone());
                let owner = user.as_str().to_owned();
                let path = as_user(identity, move || share.restore(&path, &owner)).await?;
                Ok((Response::Restored { path }, None))
            }
            other => Err(OperationError::Refused(format!("{} is not a file operation", other.name()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::{ShareName, SharePath, VolumeNumber};

    use super::*;

    fn files(directory: &std::path::Path) -> Files {
        std::fs::write(
            directory.join("passwd"),
            "root:x:0:0::/root:/bin/bash\ndebian:x:1000:1000::/home/debian:/bin/bash\n\
             anna:x:2000:990::/nonexistent:/usr/sbin/nologin\nsneaky:x:2001:0::/:/bin/sh\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("group"),
            "root:x:0:\nzendiloos-users:x:990:\nfamily:x:2000:anna,ben\nwork:x:2002:ben\n",
        )
        .unwrap();
        let accounts = AccountFiles {
            passwd: directory.join("passwd"),
            group: directory.join("group"),
            high_water: directory.join("high-water"),
        };
        Files::new(accounts, directory.to_owned())
    }

    fn user(name: &str) -> UserName {
        UserName::parse(name).unwrap()
    }

    #[test]
    fn acts_only_as_managed_users() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        assert_eq!(
            files.identity(&user("anna")).unwrap(),
            Identity { uid: 2000, gid: 990, groups: vec![990, 2000] }
        );
        for other in ["root", "debian", "sneaky", "nobody"] {
            assert!(
                matches!(files.identity(&user(other)), Err(OperationError::NotManaged(_))),
                "{other} must be refused"
            );
        }
    }

    #[tokio::test]
    async fn refuses_before_touching_anything() {
        let directory = tempfile::tempdir().unwrap();
        let files = files(directory.path());
        let location = FileLocation {
            volume: VolumeNumber::new(1).unwrap(),
            share: ShareName::parse("photos").unwrap(),
            path: SharePath::root(),
        };
        let request = Request::ListDirectory { user: user("root"), location: location.clone() };
        assert!(matches!(files.execute(&request).await, Err(OperationError::NotManaged(_))));
        // The share is missing: refused as root, before switching.
        let request = Request::ListDirectory { user: user("anna"), location };
        assert!(matches!(files.execute(&request).await, Err(OperationError::File(FileError::NotFound))));
    }
}
