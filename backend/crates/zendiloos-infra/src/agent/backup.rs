//! [`BackupHost`] through the system agent: the Backup context's places,
//! tasks and secrets checked once more as protocol types; what rsync,
//! rclone and ssh-keyscan printed read here, unprivileged (a listing's
//! entries, a run's itemized changes, a server's host keys with their
//! fingerprints); a cloud's token rclone refreshed passed back.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    AgentTransport, BACKUP_HOST_KEY_CHANGED, BackupCloud, BackupCloudAccess, BackupCloudWord, BackupCrypt,
    BackupEnd, BackupExisting, BackupFolder, BackupHostKey, BackupHostName, BackupMode, BackupPassword,
    BackupPattern, BackupPruned, BackupRemotePath, BackupRestore, BackupRoot, BackupRun, BackupSecret,
    BackupSecrets, BackupServer, BackupStamp, BackupTaskId, BackupTransport, BackupWord, ErrorCode,
    PackageRunState, PackageTask, RemovableId, Report, Request, Response, ShareLocation, ShareName,
    SharePath, VolumeNumber,
};
use zendiloos_backup::application::{
    BackupHost, HostError, HostFacts, HostKey, Listing, Location, RestorePlan, RestoreReport, RunMode,
    RunPlan, RunReport, Secrets, Steps,
};
use zendiloos_backup::domain::{Entry, EntryKind, Existing, Provider, Server, Transfer, Transport};
use zendiloos_kernel::{PortError, RequestContext, Secret, UtcTime};

use super::{WHAT, agent_error, call, call_raw, call_raw_reporting, unexpected};

const PACKAGES_POLL: Duration = Duration::from_secs(2);
const PACKAGES_TIMEOUT: Duration = Duration::from_secs(3600);

pub struct AgentBackup {
    transport: Arc<dyn AgentTransport>,
}

impl AgentBackup {
    /// `transport` waits as long as a backup may take (a day).
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }

    fn transport(&self) -> &dyn AgentTransport {
        self.transport.as_ref()
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

/// The agent's answer as the context hears it: a changed host key told
/// apart from other refusals.
fn host_answer(response: Response) -> Result<Response, HostError> {
    match response {
        Response::Error { code: ErrorCode::Forbidden | ErrorCode::Conflict, message } => {
            let message = message.trim_start_matches("refused: ").to_owned();
            match message.strip_prefix(BACKUP_HOST_KEY_CHANGED) {
                Some(detail) => Err(HostError::HostKeyChanged(detail.trim_start_matches(": ").to_owned())),
                None => Err(HostError::Refused(message)),
            }
        }
        Response::Error { code: ErrorCode::Failed, message } => Err(HostError::Failed(message)),
        Response::Error { code, message } => Err(agent_error(code, &message).into()),
        other => Ok(other),
    }
}

fn secret(secret: &Secret, what: &str) -> Result<BackupSecret, HostError> {
    BackupSecret::new(secret.expose()).map_err(|_| invalid(what))
}

fn word(word: &str, what: &str) -> Result<BackupCloudWord, HostError> {
    BackupCloudWord::parse(word).map_err(|_| invalid(what))
}

/// What the unit needs, checked; a crypt only for what is `encrypted`.
fn secrets(secrets: &Secrets, encrypted: bool) -> Result<BackupSecrets, HostError> {
    let password = secrets
        .password
        .as_ref()
        .map(|s| BackupPassword::new(s.expose()).map_err(|_| invalid("password")))
        .transpose()?;
    let crypt = match (&secrets.crypt, encrypted) {
        (Some(crypt), true) => Some(BackupCrypt {
            password: secret(&crypt.password, "crypt's password")?,
            salt: crypt.salt.as_ref().map(|salt| secret(salt, "crypt's salt")).transpose()?,
        }),
        (None, true) => {
            return Err(HostError::Refused("the task is encrypted, but its password is missing".into()));
        }
        (_, false) => None,
    };
    let cloud = secrets
        .cloud
        .as_ref()
        .map(|cloud| {
            Ok::<_, HostError>(BackupCloudAccess {
                token: secret(&cloud.token, "cloud's token")?,
                client_id: cloud.client_id.as_deref().map(|id| word(id, "client id")).transpose()?,
                client_secret: cloud
                    .client_secret
                    .as_ref()
                    .map(|s| secret(s, "client secret"))
                    .transpose()?,
                drive_id: cloud.drive_id.as_deref().map(|id| word(id, "drive id")).transpose()?,
                drive_type: cloud.drive_type.as_deref().map(|kind| word(kind, "drive type")).transpose()?,
            })
        })
        .transpose()?;
    Ok(BackupSecrets { password, crypt, cloud })
}

fn kept(token: Option<BackupSecret>) -> Option<Secret> {
    token.map(|token| Secret::new(token.expose()))
}

fn server(server: &Server) -> Result<BackupServer, HostError> {
    let transport = match &server.transport {
        Transport::Daemon { module } => {
            BackupTransport::Daemon { module: BackupWord::parse(module).map_err(|_| invalid("module"))? }
        }
        Transport::Ssh { host_key, .. } => BackupTransport::Ssh {
            host_key: BackupHostKey::parse(host_key).map_err(|_| invalid("host key"))?,
        },
    };
    Ok(BackupServer {
        transport,
        host: BackupHostName::parse(&server.host).map_err(|_| invalid("host"))?,
        port: server.port,
        user: BackupWord::parse(&server.user).map_err(|_| invalid("user"))?,
    })
}

/// A place in the agent's terms: every value checked once more.
fn end(location: &Location) -> Result<BackupEnd, HostError> {
    let path = |path: &str| SharePath::parse(path).map_err(|_| invalid("path"));
    Ok(match location {
        Location::Share { volume, share, path: folder } => BackupEnd::Nas {
            root: BackupRoot::Share {
                location: ShareLocation {
                    volume: VolumeNumber::new(*volume).map_err(|_| invalid("volume"))?,
                    share: ShareName::parse(share).map_err(|_| invalid("share"))?,
                },
            },
            path: path(folder)?,
        },
        Location::Drive { drive, path: folder } => BackupEnd::Nas {
            root: BackupRoot::Drive { drive: RemovableId::parse(drive).map_err(|_| invalid("drive"))? },
            path: path(folder)?,
        },
        Location::Remote { server: remote, path: folder } => BackupEnd::Remote {
            server: server(remote)?,
            path: BackupRemotePath::parse(folder).map_err(|_| invalid("folder on a server"))?,
        },
        Location::Cloud { provider, path: folder, .. } => BackupEnd::Cloud {
            cloud: match provider {
                Provider::GoogleDrive => BackupCloud::GoogleDrive,
                Provider::OneDrive => BackupCloud::OneDrive,
            },
            path: path(folder)?,
        },
    })
}

/// A task's folder, split into the folder it is in and its own name, as a
/// run names them.
fn split_folder(location: &Location) -> Result<(Location, BackupFolder), HostError> {
    let path = location.path();
    let (parent, name) = match path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_owned(), name),
        None => (String::new(), path),
    };
    let folder = BackupFolder::parse(name).map_err(|_| invalid("task's folder"))?;
    let parent = match location {
        Location::Share { volume, share, .. } => {
            Location::Share { volume: *volume, share: share.clone(), path: parent }
        }
        Location::Drive { drive, .. } => Location::Drive { drive: drive.clone(), path: parent },
        Location::Remote { server, path } => {
            let parent = if parent.is_empty() && path.starts_with('/') { "/".to_owned() } else { parent };
            Location::Remote { server: server.clone(), path: parent }
        }
        Location::Cloud { provider, account, .. } => {
            Location::Cloud { provider: *provider, account: *account, path: parent }
        }
    };
    Ok((parent, folder))
}

/// rsync writes characters it would not print as `\#ooo` (octal).
fn unescape(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\'
            && bytes.get(at + 1) == Some(&b'#')
            && let Some(code) = name.get(at + 2..at + 5).and_then(|digits| u8::from_str_radix(digits, 8).ok())
        {
            out.push(code);
            at += 5;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One line of `rsync --list-only`: mode, size, date and time (in UTC, as
/// the unit runs rsync), name.
pub(crate) fn listed(line: &str) -> Option<Entry> {
    let mut rest = line;
    let mut fields = Vec::new();
    for _ in 0..4 {
        rest = rest.trim_start();
        let end = rest.find(' ')?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let (mode, size, date, time) = (fields[0], fields[1], fields[2], fields[3]);
    let name = rest.strip_prefix(' ')?;
    if mode.len() != 10 || name.is_empty() {
        return None;
    }
    let kind = match mode.as_bytes()[0] {
        b'd' => EntryKind::Folder,
        b'-' => EntryKind::File,
        b'l' => EntryKind::Link,
        _ => EntryKind::Other,
    };
    let name = if kind == EntryKind::Link { name.split(" -> ").next().unwrap_or(name) } else { name };
    let bytes = size.chars().filter(char::is_ascii_digit).collect::<String>().parse().ok()?;
    let number = |text: &str| text.parse::<i64>().ok();
    let modified = {
        let mut d = date.split('/');
        let mut t = time.split(':');
        let next = |parts: &mut std::str::Split<'_, char>| parts.next().and_then(number);
        let fields = (next(&mut d), next(&mut d), next(&mut d), next(&mut t), next(&mut t), next(&mut t));
        match fields {
            (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) => UtcTime {
                year,
                month: u8::try_from(month).ok()?,
                day: u8::try_from(day).ok()?,
                hour: u8::try_from(hour).ok()?,
                minute: u8::try_from(minute).ok()?,
                second: u8::try_from(second).ok()?,
            }
            .timestamp(),
            _ => None,
        }
    };
    Some(Entry { name: unescape(name), kind, bytes, modified })
}

fn entries(listing: &str) -> Vec<Entry> {
    listing.lines().filter_map(listed).collect()
}

/// An itemized change, `>f+++++++++ a.jpg` or `*deleting   old.txt`.
fn transfer(line: &str) -> Option<Transfer> {
    if let Some(path) = line.strip_prefix("*deleting") {
        return Some(Transfer { change: "*deleting".into(), path: unescape(path.trim_start()) });
    }
    let (change, path) = line.split_at_checked(11)?;
    let path = path.strip_prefix(' ')?;
    Some(Transfer { change: change.to_owned(), path: unescape(path) })
}

/// The host keys `ssh-keyscan` printed (`host type key`), with their
/// fingerprints as `ssh` shows them: SHA-256 of the key, in base64.
pub(crate) fn host_keys(keyscan: &str) -> Vec<HostKey> {
    let mut keys: Vec<HostKey> = keyscan
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (_, kind, key) = (words.next()?, words.next()?, words.next()?);
            let line = format!("{kind} {key}");
            BackupHostKey::parse(&line).ok()?;
            let blob = base64::engine::general_purpose::STANDARD.decode(key).ok()?;
            let digest = Sha256::digest(&blob);
            let fingerprint =
                format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest));
            Some(HostKey { line, fingerprint })
        })
        .collect();
    // ed25519 first, as ssh prefers it.
    keys.sort_by_key(|key| !key.line.starts_with("ssh-ed25519"));
    keys.dedup();
    keys
}

struct Forward<'a>(&'a dyn Steps);

impl Report for Forward<'_> {
    fn step(&self, step: &str, percent: Option<u8>) {
        self.0.step(step, percent);
    }
}

#[async_trait]
impl BackupHost for AgentBackup {
    async fn facts(&self, context: &RequestContext) -> Result<HostFacts, PortError> {
        match call(self.transport(), Request::BackupFacts {}, context).await? {
            Response::BackupFacts { rsync, ssh, rclone, public_key } => {
                Ok(HostFacts { rsync, ssh, rclone, public_key })
            }
            other => Err(unexpected(other)),
        }
    }

    async fn install(&self, context: &RequestContext) -> Result<(), HostError> {
        let request = Request::StartPackages { task: PackageTask::InstallBackup, release: None };
        let id = match host_answer(call_raw(self.transport(), request, context).await?)? {
            Response::PackagesStarted { id } => id,
            other => return Err(unexpected(other).into()),
        };
        // apt records its run; looked at until it ends, for an hour at most.
        let deadline = tokio::time::Instant::now() + PACKAGES_TIMEOUT;
        loop {
            tokio::time::sleep(PACKAGES_POLL).await;
            let run = match call(self.transport(), Request::PackagesState {}, context).await? {
                Response::Packages { run, .. } => run,
                other => return Err(unexpected(other).into()),
            };
            match run {
                Some(run) if run.id == id && run.state == PackageRunState::Succeeded => return Ok(()),
                Some(run) if run.id == id && run.state == PackageRunState::Failed => {
                    return Err(HostError::Failed(run.error.unwrap_or_else(|| "apt failed".into())));
                }
                _ if tokio::time::Instant::now() > deadline => {
                    return Err(HostError::Failed("apt did not finish within an hour".into()));
                }
                _ => {}
            }
        }
    }

    async fn make_key(&self, context: &RequestContext) -> Result<String, HostError> {
        match host_answer(call_raw(self.transport(), Request::MakeBackupKey {}, context).await?)? {
            Response::BackupKey { public_key } => Ok(public_key),
            other => Err(unexpected(other).into()),
        }
    }

    async fn host_keys(
        &self,
        host: &str,
        port: u16,
        context: &RequestContext,
    ) -> Result<Vec<HostKey>, HostError> {
        let request = Request::ScanBackupHostKeys {
            host: BackupHostName::parse(host).map_err(|_| invalid("host"))?,
            port,
        };
        match host_answer(call_raw(self.transport(), request, context).await?)? {
            Response::BackupHostKeys { keyscan } => Ok(host_keys(&keyscan)),
            other => Err(unexpected(other).into()),
        }
    }

    async fn list(
        &self,
        at: &Location,
        inside: &str,
        secrets: &Secrets,
        context: &RequestContext,
    ) -> Result<Listing, HostError> {
        let request = Request::ListBackup {
            at: end(at)?,
            inside: SharePath::parse(inside).map_err(|_| invalid("path"))?,
            secrets: self::secrets(secrets, secrets.crypt.is_some())?,
        };
        match host_answer(call_raw(self.transport(), request, context).await?)? {
            Response::BackupListing { missing: true, token, .. } => {
                Ok(Listing { entries: Vec::new(), token: kept(token) })
            }
            Response::BackupListing { listing, token, .. } => {
                Ok(Listing { entries: entries(&listing), token: kept(token) })
            }
            other => Err(unexpected(other).into()),
        }
    }

    async fn run(
        &self,
        plan: &RunPlan,
        secrets: &Secrets,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<RunReport, HostError> {
        let (target, folder) = split_folder(&plan.target)?;
        let excludes = plan
            .filters
            .excludes
            .iter()
            .map(|pattern| BackupPattern::parse(pattern).map_err(|_| invalid("exclude pattern")))
            .collect::<Result<Vec<_>, _>>()?;
        let run = BackupRun {
            task: task_id(plan.task)?,
            source: end(&plan.source)?,
            target: end(&target)?,
            folder,
            mode: match plan.mode {
                RunMode::Mirror { keep_changed } => BackupMode::Mirror { keep_changed },
                RunMode::Versioned => BackupMode::Versioned,
            },
            stamp: BackupStamp::parse(&plan.stamp).map_err(|_| invalid("version's name"))?,
            excludes,
            hidden: plan.filters.hidden,
            bwlimit_kib: plan.bwlimit_kib,
        };
        let request = Request::RunBackup { run, secrets: self::secrets(secrets, plan.encrypted)? };
        match host_answer(call_raw_reporting(self.transport(), request, &Forward(steps), context).await?)? {
            Response::BackupRan { files, bytes, partial, transfers, transfers_total, listing, token } => {
                Ok(RunReport {
                    files,
                    bytes,
                    partial,
                    transfers: transfers.iter().filter_map(|line| transfer(line)).collect(),
                    transfers_total,
                    entries: entries(&listing),
                    token: kept(token),
                })
            }
            other => Err(unexpected(other).into()),
        }
    }

    async fn stop(&self, task: i64, context: &RequestContext) -> Result<(), HostError> {
        match host_answer(
            call_raw(self.transport(), Request::StopBackup { task: task_id(task)? }, context).await?,
        )? {
            Response::Done => Ok(()),
            other => Err(unexpected(other).into()),
        }
    }

    async fn prune(
        &self,
        task: i64,
        at: &Location,
        names: &[String],
        secrets: &Secrets,
        context: &RequestContext,
    ) -> Result<Option<Secret>, HostError> {
        let names = names
            .iter()
            .map(|name| BackupPruned::parse(name).map_err(|_| invalid("version to remove")))
            .collect::<Result<Vec<_>, _>>()?;
        let secrets = self::secrets(secrets, secrets.crypt.is_some())?;
        let request = Request::PruneBackup { task: task_id(task)?, at: end(at)?, names, secrets };
        match host_answer(call_raw(self.transport(), request, context).await?)? {
            Response::BackupPruned { token } => Ok(kept(token)),
            other => Err(unexpected(other).into()),
        }
    }

    async fn restore(
        &self,
        plan: &RestorePlan,
        secrets: &Secrets,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<RestoreReport, HostError> {
        let paths = plan
            .paths
            .iter()
            .map(|path| SharePath::parse(path).map_err(|_| invalid("path to restore")))
            .collect::<Result<Vec<_>, _>>()?;
        // The task's folder, where an encryption begins, and the version in it.
        let (folder, version) = split_folder(&plan.from)?;
        let restore = BackupRestore {
            task: task_id(plan.task)?,
            from: end(&folder)?,
            within: SharePath::parse(version.as_str()).map_err(|_| invalid("version"))?,
            paths,
            to: end(&plan.to)?,
            existing: match plan.existing {
                Existing::Skip => BackupExisting::Skip,
                Existing::Replace => BackupExisting::Replace,
            },
        };
        let request = Request::RestoreBackup { restore, secrets: self::secrets(secrets, plan.encrypted)? };
        match host_answer(call_raw_reporting(self.transport(), request, &Forward(steps), context).await?)? {
            Response::BackupRestored { files, bytes, partial, token } => {
                Ok(RestoreReport { files, bytes, partial, token: kept(token) })
            }
            other => Err(unexpected(other).into()),
        }
    }
}

fn task_id(task: i64) -> Result<BackupTaskId, HostError> {
    u32::try_from(task).ok().and_then(|id| BackupTaskId::new(id).ok()).ok_or_else(|| invalid("task's number"))
}

#[cfg(test)]
mod tests;
