//! Use cases on shares. Every change keeps three places in step: the
//! database, the share's directory (dataset and ACLs) and the SMB and NFS
//! servers' configuration, which is always written as a whole from the
//! database.

use std::collections::HashSet;
use std::sync::Arc;

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, AuditEntry, Outcome, PortError, RequestContext};

use super::SharingPorts;
use super::ports::{HostError, Names};
use crate::domain::{
    Access, AccessRule, AppAccess, InvalidNetwork, InvalidShareName, NewShare, NfsClient, NfsSpec, Principal,
    Share, ShareId, ShareName, ShareSettings, ShareUsage, SharingConfig, SmbSpec, Subject, network,
};

pub const MAX_DESCRIPTION_CHARS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SharingError {
    #[error(transparent)]
    InvalidName(#[from] InvalidShareName),
    #[error(transparent)]
    InvalidNetwork(#[from] InvalidNetwork),
    #[error("the description must be at most {MAX_DESCRIPTION_CHARS} characters, without line breaks")]
    InvalidDescription,
    #[error("a share with this name exists")]
    NameTaken,
    #[error("there is no such volume")]
    VolumeNotFound,
    #[error("there is no such share")]
    ShareNotFound,
    #[error("an account or group in the rules does not exist")]
    UnknownSubject,
    #[error("an account, group or network is listed twice")]
    Duplicate,
    #[error("the confirmation does not match the share's name")]
    ConfirmationMismatch,
    /// Apps mount the share; they are named.
    #[error("apps use this share: {0}")]
    UsedByApps(String),
    #[error("refused by the host: {0}")]
    Refused(String),
    #[error(transparent)]
    InvalidSettings(#[from] crate::domain::InvalidSettings),
    #[error(transparent)]
    InvalidSchedule(#[from] crate::domain::InvalidSchedule),
    #[error(transparent)]
    InvalidNote(#[from] crate::domain::InvalidNote),
    #[error("there is no such snapshot")]
    SnapshotNotFound,
    /// Snapshots are named after the second they are taken in.
    #[error("a snapshot was taken this second; try again")]
    SnapshotTaken,
    #[error(transparent)]
    Port(#[from] PortError),
}

impl From<HostError> for SharingError {
    fn from(error: HostError) -> Self {
        match error {
            HostError::Refused(reason) => SharingError::Refused(reason),
            HostError::Conflict => SharingError::NameTaken,
            HostError::Port(error) => SharingError::Port(error),
        }
    }
}

pub struct NewShareInput {
    pub name: String,
    pub description: Option<String>,
    /// Name of the pool (volume) to create it on.
    pub pool: String,
    pub recycle_bin: bool,
    pub access: Vec<AccessRule>,
    /// Networks as typed; validated here.
    pub nfs: Vec<(String, bool)>,
    pub settings: ShareSettings,
}

/// Changes to a share; `None` leaves a field as it is.
#[derive(Default)]
pub struct ShareChanges {
    pub description: Option<Option<String>>,
    pub recycle_bin: Option<bool>,
    pub access: Option<Vec<AccessRule>>,
    pub nfs: Option<Vec<(String, bool)>>,
    pub settings: Option<ShareSettings>,
}

fn description(value: Option<&str>) -> Result<Option<String>, SharingError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if value.chars().count() > MAX_DESCRIPTION_CHARS || value.chars().any(char::is_control) {
        return Err(SharingError::InvalidDescription);
    }
    Ok(Some(value.to_owned()))
}

fn clients(input: Vec<(String, bool)>) -> Result<Vec<NfsClient>, SharingError> {
    let mut clients: Vec<NfsClient> = Vec::new();
    for (value, write) in input {
        let normalized = network(&value)?;
        if clients.iter().any(|c| c.network == normalized) {
            return Err(SharingError::Duplicate);
        }
        clients.push(NfsClient { network: normalized, write });
    }
    Ok(clients)
}

fn check_rules(rules: &[AccessRule], names: &Names) -> Result<(), SharingError> {
    let mut seen = HashSet::new();
    for rule in rules {
        if !seen.insert(rule.subject) {
            return Err(SharingError::Duplicate);
        }
        let known = match rule.subject {
            Subject::Account(id) => names.accounts.contains_key(&id),
            Subject::Group(id) => names.groups.contains_key(&id),
        };
        if !known {
            return Err(SharingError::UnknownSubject);
        }
    }
    Ok(())
}

/// Rules with names; rules of accounts or groups deleted since are left out.
fn principals(rules: &[AccessRule], names: &Names) -> Vec<(Principal, Access)> {
    rules
        .iter()
        .filter_map(|rule| {
            let principal = match rule.subject {
                Subject::Account(id) => Principal::User(names.accounts.get(&id)?.clone()),
                Subject::Group(id) => Principal::Group(names.groups.get(&id)?.clone()),
            };
            Some((principal, rule.access))
        })
        .collect()
}

/// What the share's directory should allow: the people's rules, then the
/// apps'. Samba never learns of the apps.
fn acl(rules: &[AccessRule], names: &Names, share: ShareId, apps: &[AppAccess]) -> Vec<(Principal, Access)> {
    let mut acl = principals(rules, names);
    acl.extend(
        apps.iter()
            .filter(|app| app.share == share)
            .map(|app| (Principal::User(app.account.clone()), app.access)),
    );
    acl
}

/// What the servers should offer for `shares`.
pub fn config(shares: &[Share], names: &Names) -> SharingConfig {
    SharingConfig {
        smb: shares
            .iter()
            .map(|share| SmbSpec {
                name: share.name.clone(),
                volume: share.volume,
                access: principals(&share.access, names),
                recycle_bin: share.recycle_bin,
                comment: share.description.clone(),
            })
            .collect(),
        nfs: shares
            .iter()
            .filter(|share| !share.nfs.is_empty())
            .map(|share| NfsSpec {
                name: share.name.clone(),
                volume: share.volume,
                clients: share.nfs.clone(),
            })
            .collect(),
    }
}

/// All use cases of this context. Cheap to clone.
#[derive(Clone)]
pub struct SharingUseCases {
    pub(super) ports: SharingPorts,
    /// Changes and server configuration, one at a time.
    pub(super) changes: Arc<tokio::sync::Mutex<()>>,
    /// Shares whose scheduled snapshot failed; reported once until one works.
    pub(super) snapshot_failures: Arc<std::sync::Mutex<std::collections::HashSet<ShareId>>>,
}

impl SharingUseCases {
    pub fn new(ports: SharingPorts) -> Self {
        Self { ports, changes: Arc::new(tokio::sync::Mutex::new(())), snapshot_failures: Arc::default() }
    }

    pub async fn list(&self) -> Result<Vec<Share>, SharingError> {
        Ok(self.ports.shares.list().await?)
    }

    /// Writes the servers' configuration from the database, e.g. at startup
    /// or after accounts or groups changed.
    pub async fn apply(&self, request: &RequestContext) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        self.apply_locked(request).await?;
        // The datasets get what the database says, whatever changed them.
        for share in self.ports.shares.list().await? {
            if let Err(error) = self
                .ports
                .host
                .set_properties(&share.pool, share.volume, &share.name, &share.settings, request)
                .await
            {
                tracing::warn!(%error, share = share.name.as_str(), "cannot set the share's properties");
            }
        }
        Ok(())
    }

    /// What each share holds and may still take; shares whose volume is
    /// not there are left out.
    pub async fn usage(&self, request: &RequestContext) -> Result<Vec<(ShareId, ShareUsage)>, SharingError> {
        let shares = self.ports.shares.list().await?;
        let usage = self.ports.host.usage(request).await?;
        Ok(shares
            .iter()
            .filter_map(|share| {
                let found = usage.iter().find(|u| u.pool == share.pool && u.name == share.name)?;
                Some((share.id, found.clone()))
            })
            .collect())
    }

    async fn apply_locked(&self, request: &RequestContext) -> Result<(), SharingError> {
        let shares = self.ports.shares.list().await?;
        let names = self.ports.directory.names().await?;
        self.ports.host.apply(&config(&shares, &names), request).await?;
        Ok(())
    }

    pub(super) async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry =
            AuditEntry::new(self.ports.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    /// Creates the dataset, sets its permissions, stores the share and
    /// offers it. A failure after the dataset exists removes it again.
    pub async fn create(
        &self,
        input: NewShareInput,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Share, SharingError> {
        let name = ShareName::parse(input.name.trim())?;
        let description = description(input.description.as_deref())?;
        let nfs = clients(input.nfs)?;
        input.settings.check()?;

        let _changes = self.changes.lock().await;
        let names = self.ports.directory.names().await?;
        check_rules(&input.access, &names)?;
        if self.ports.shares.list().await?.iter().any(|share| share.name == name) {
            return Err(SharingError::NameTaken);
        }
        let volumes = self.ports.volumes.volumes(request).await?;
        let volume = volumes
            .iter()
            .find(|v| v.pool == input.pool && v.mounted)
            .ok_or(SharingError::VolumeNotFound)?
            .number;

        self.ports.host.create_dataset(&input.pool, volume, &name, request).await?;
        let stored = async {
            self.ports.host.set_properties(&input.pool, volume, &name, &input.settings, request).await?;
            let access = principals(&input.access, &names);
            self.ports.host.set_access(volume, &name, &access, &[], false, request).await?;
            let new = NewShare {
                name: name.clone(),
                description,
                pool: input.pool.clone(),
                volume,
                recycle_bin: input.recycle_bin,
                settings: input.settings,
                access: input.access,
                nfs,
                created_at: self.ports.clock.now(),
            };
            let share = self.ports.shares.create(new).await?.ok_or(SharingError::NameTaken)?;
            self.apply_locked(request).await?;
            Ok::<_, SharingError>(share)
        }
        .await;
        match stored {
            Ok(share) => {
                self.audit("sharing.share_create", Outcome::Success, name.as_str(), actor, request).await;
                Ok(share)
            }
            Err(error) => {
                if let Err(cleanup) =
                    self.ports.host.destroy_dataset(&input.pool, volume, &name, request).await
                {
                    tracing::error!(share = name.as_str(), error = %cleanup, "cannot remove the dataset of a failed share");
                }
                Err(error)
            }
        }
    }

    /// Changes a share. New access rules apply to the share's directory at
    /// once; existing files follow in a job, returned with the share.
    pub async fn update(
        &self,
        id: ShareId,
        changes: ShareChanges,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(Share, Option<Job>), SharingError> {
        let description = changes.description.map(|d| description(d.as_deref())).transpose()?;
        let nfs = changes.nfs.map(clients).transpose()?;

        let _changes = self.changes.lock().await;
        let mut share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        let names = self.ports.directory.names().await?;
        let mut follow_up = None;
        if let Some(access) = changes.access
            && access != share.access
        {
            check_rules(&access, &names)?;
            let apps = self.ports.shares.app_access().await?;
            let now = acl(&access, &names, share.id, &apps);
            let before = acl(&share.access, &names, share.id, &apps);
            let removed: Vec<Principal> =
                before.into_iter().map(|(p, _)| p).filter(|p| !now.iter().any(|(q, _)| q == p)).collect();
            self.ports.host.set_access(share.volume, &share.name, &now, &removed, false, request).await?;
            share.access = access;
            follow_up = Some((now, removed));
        }
        if let Some(settings) = changes.settings
            && settings != share.settings
        {
            let used = self
                .ports
                .host
                .usage(request)
                .await?
                .into_iter()
                .find(|u| u.pool == share.pool && u.name == share.name)
                .map_or(0, |u| u.used_bytes);
            settings.check_against(used)?;
            self.ports
                .host
                .set_properties(&share.pool, share.volume, &share.name, &settings, request)
                .await?;
            share.settings = settings;
        }
        if let Some(description) = description {
            share.description = description;
        }
        if let Some(recycle_bin) = changes.recycle_bin {
            share.recycle_bin = recycle_bin;
        }
        if let Some(nfs) = nfs {
            share.nfs = nfs;
        }
        share.updated_at = self.ports.clock.now();
        self.ports.shares.update(&share).await?;
        self.apply_locked(request).await?;
        self.audit("sharing.share_update", Outcome::Success, share.name.as_str(), actor, request).await;

        let job = match follow_up {
            Some((access, removed)) => {
                let host = Arc::clone(&self.ports.host);
                let (volume, name, request) = (share.volume, share.name.clone(), request.clone());
                let job = self
                    .ports
                    .jobs
                    .start(
                        "sharing.apply_access",
                        Some(share.name.to_string()),
                        Some(actor),
                        move |progress| async move {
                            progress.step("existing_files", None).await;
                            host.set_access(volume, &name, &access, &removed, true, &request)
                                .await
                                .map_err(|error| JobError::new("agent_failed", Some(error.to_string())))
                        },
                    )
                    .await?;
                Some(job)
            }
            None => None,
        };
        Ok((share, job))
    }

    /// Gives an app's `account` exactly `grants` (share by name), on the
    /// share's directory at once and then on everything in it; shares it
    /// had before and has no more lose the account's entries. Runs to the
    /// end: the caller is the app's own job.
    pub async fn set_app_access(
        &self,
        account: &str,
        grants: &[(ShareName, Access)],
        request: &RequestContext,
    ) -> Result<(), SharingError> {
        let mut follow_up = Vec::new();
        {
            let _changes = self.changes.lock().await;
            let shares = self.ports.shares.list().await?;
            let mut wanted = Vec::new();
            for (name, access) in grants {
                let share = shares.iter().find(|s| &s.name == name).ok_or(SharingError::ShareNotFound)?;
                wanted.push((share.id, *access));
            }
            let before = self.ports.shares.app_access().await?;
            self.ports.shares.set_app_access(account, &wanted).await?;
            let after = self.ports.shares.app_access().await?;
            let names = self.ports.directory.names().await?;
            let touched: HashSet<ShareId> = before
                .iter()
                .filter(|app| app.account == account)
                .map(|app| app.share)
                .chain(wanted.iter().map(|(id, _)| *id))
                .collect();
            for share in shares.iter().filter(|share| touched.contains(&share.id)) {
                let now = acl(&share.access, &names, share.id, &after);
                let removed: Vec<Principal> = if wanted.iter().any(|(id, _)| *id == share.id) {
                    Vec::new()
                } else {
                    vec![Principal::User(account.to_owned())]
                };
                self.ports.host.set_access(share.volume, &share.name, &now, &removed, false, request).await?;
                follow_up.push((share.volume, share.name.clone(), now, removed));
            }
        }
        for (volume, name, now, removed) in follow_up {
            self.ports.host.set_access(volume, &name, &now, &removed, true, request).await?;
        }
        Ok(())
    }

    /// Stops offering a share and destroys its dataset with all files, as
    /// a job. `confirm` must repeat the share's name.
    pub async fn delete(
        &self,
        id: ShareId,
        confirm: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, SharingError> {
        let _changes = self.changes.lock().await;
        let share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        if confirm.trim() != share.name.as_str() {
            return Err(SharingError::ConfirmationMismatch);
        }
        let users: Vec<String> = self
            .ports
            .shares
            .app_access()
            .await?
            .into_iter()
            .filter(|app| app.share == share.id)
            .map(|app| app.account.strip_prefix("app-").unwrap_or(&app.account).to_owned())
            .collect();
        if !users.is_empty() {
            return Err(SharingError::UsedByApps(users.join(", ")));
        }
        // Not offered any more before its data goes.
        self.ports.shares.delete(id).await?;
        self.apply_locked(request).await?;

        let this = self.clone();
        let (actor_owned, request_owned) = (actor.clone(), request.clone());
        let job = self
            .ports
            .jobs
            .start(
                "sharing.delete_share",
                Some(share.name.to_string()),
                Some(actor),
                move |progress| async move {
                    progress.step("destroying", None).await;
                    let outcome = this
                        .ports
                        .host
                        .destroy_dataset(&share.pool, share.volume, &share.name, &request_owned)
                        .await;
                    let audit = if outcome.is_ok() { Outcome::Success } else { Outcome::Failure };
                    this.audit(
                        "sharing.share_delete",
                        audit,
                        share.name.as_str(),
                        &actor_owned,
                        &request_owned,
                    )
                    .await;
                    outcome.map_err(|error| JobError::new("agent_failed", Some(error.to_string())))
                },
            )
            .await?;
        Ok(job)
    }
}
