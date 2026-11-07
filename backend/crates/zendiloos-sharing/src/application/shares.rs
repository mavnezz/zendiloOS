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
    Access, AccessRule, AppAccess, FileServices, HomesSpec, InvalidFileServices, InvalidNetwork,
    InvalidShareName, NewShare, NfsClient, NfsSpec, Permission, Principal, Share, ShareId, ShareName,
    ShareSettings, ShareUsage, SharingConfig, SmbSpec, Subject, network,
};

pub const MAX_DESCRIPTION_CHARS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SharingError {
    #[error(transparent)]
    InvalidName(#[from] InvalidShareName),
    #[error(transparent)]
    InvalidNetwork(#[from] InvalidNetwork),
    #[error(transparent)]
    InvalidServices(#[from] InvalidFileServices),
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

/// Rules with names, groups as groups, for Samba, which follows their
/// members itself; rules of accounts or groups deleted since are left out.
fn principals(rules: &[AccessRule], names: &Names) -> Vec<(Principal, Permission)> {
    rules
        .iter()
        .filter_map(|rule| {
            let principal = match rule.subject {
                Subject::Account(id) => Principal::User(names.accounts.get(&id)?.clone()),
                Subject::Group(id) => Principal::Group(names.groups.get(&id)?.clone()),
            };
            Some((principal, rule.permission))
        })
        .collect()
}

/// The accounts the rules deny, themselves or through a group, sorted.
fn denied(rules: &[AccessRule], names: &Names) -> Vec<i64> {
    let mut denied: Vec<i64> = rules
        .iter()
        .filter(|rule| rule.permission == Permission::Deny)
        .flat_map(|rule| match rule.subject {
            Subject::Account(id) => vec![id],
            Subject::Group(id) => names.members.get(&id).cloned().unwrap_or_default(),
        })
        .filter(|id| names.accounts.contains_key(id))
        .collect();
    denied.sort_unstable();
    denied.dedup();
    denied
}

/// What the share's directory should say, as POSIX ACL entries: the
/// grants of accounts not denied and of groups, an entry without
/// permissions for every account denied, itself or as a member of a
/// group (a named user entry comes before the group entries), then the
/// apps'. Samba never learns of the apps.
pub(super) fn acl(
    rules: &[AccessRule],
    names: &Names,
    share: ShareId,
    apps: &[AppAccess],
) -> Vec<(Principal, Permission)> {
    let denied = denied(rules, names);
    let mut acl: Vec<(Principal, Permission)> = principals(
        &rules
            .iter()
            .filter(|rule| rule.permission != Permission::Deny)
            .filter(|rule| !matches!(rule.subject, Subject::Account(id) if denied.contains(&id)))
            .cloned()
            .collect::<Vec<_>>(),
        names,
    );
    acl.extend(
        denied
            .iter()
            .filter_map(|id| Some((Principal::User(names.accounts.get(id)?.clone()), Permission::Deny))),
    );
    acl.extend(
        apps.iter()
            .filter(|app| app.share == share)
            .map(|app| (Principal::User(app.account.clone()), app.access.into())),
    );
    acl
}

/// What the servers should offer for `shares`.
pub fn config(
    shares: &[Share],
    names: &Names,
    services: FileServices,
    homes: Option<HomesSpec>,
) -> SharingConfig {
    SharingConfig {
        services: services.clone(),
        homes,
        smb: shares
            .iter()
            .map(|share| SmbSpec {
                name: share.name.clone(),
                volume: share.volume,
                access: principals(&share.access, names),
                recycle_bin: share.recycle_bin,
                comment: share.description.clone(),
                time_machine: services
                    .time_machine
                    .filter(|machine| machine.share == share.id)
                    .map(|machine| machine.max_bytes),
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

    pub(super) async fn apply_locked(&self, request: &RequestContext) -> Result<(), SharingError> {
        let shares = self.ports.shares.list().await?;
        let names = self.ports.directory.names().await?;
        let services = self.ports.shares.file_services().await?;
        let homes = match self.ports.shares.homes_location().await? {
            Some(location) => {
                let mut users: Vec<String> = self
                    .ports
                    .shares
                    .homes()
                    .await?
                    .iter()
                    .filter_map(|home| names.accounts.get(&home.account).cloned())
                    .collect();
                users.sort();
                Some(HomesSpec { volume: location.volume, users })
            }
            None => None,
        };
        self.ports.host.apply(&config(&shares, &names, services, homes), request).await?;
        Ok(())
    }

    /// The file services; a Time Machine share deleted since is gone.
    pub async fn file_services(&self) -> Result<FileServices, SharingError> {
        let mut services = self.ports.shares.file_services().await?;
        if let Some(machine) = services.time_machine
            && self.ports.shares.find(machine.share).await?.is_none()
        {
            services.time_machine = None;
        }
        Ok(services)
    }

    /// Stores the file services and applies them with the shares. If the
    /// host refuses, the previous ones return.
    pub async fn set_file_services(
        &self,
        services: FileServices,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<FileServices, SharingError> {
        let services = services.checked()?;
        let _changes = self.changes.lock().await;
        if let Some(machine) = services.time_machine
            && self.ports.shares.find(machine.share).await?.is_none()
        {
            return Err(SharingError::ShareNotFound);
        }
        let previous = self.ports.shares.file_services().await?;
        self.ports.shares.set_file_services(&services).await?;
        let applied = self.apply_locked(request).await;
        if applied.is_err() {
            self.ports.shares.set_file_services(&previous).await?;
            if let Err(error) = self.apply_locked(request).await {
                tracing::error!(%error, "cannot apply the previous file services again");
            }
        }
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = format!(
            "smb={} nfs={} workgroup={} min={}",
            services.smb,
            services.nfs,
            services.workgroup,
            services.smb_min_protocol.as_str()
        );
        self.audit("sharing.services_update", outcome, &target, actor, request).await;
        applied?;
        Ok(services)
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
        // A share still being deleted keeps its name until its data is gone.
        let deleting = self.ports.shares.deleting().await?;
        if self.ports.shares.list().await?.iter().chain(&deleting).any(|share| share.name == name) {
            return Err(SharingError::NameTaken);
        }
        let volumes = self.ports.volumes.volumes(request).await?;
        let volume = volumes
            .iter()
            .find(|v| v.pool == input.pool && v.mounted)
            .ok_or(SharingError::VolumeNotFound)?
            .number;

        self.ports.host.create_dataset(&input.pool, volume, &name, request).await?;
        let mut recorded = None;
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
            recorded = Some(share.id);
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
                // Neither the share nor its dataset stays: offered with no
                // dataset behind it, Samba would serve the volume's own folder.
                if let Some(id) = recorded {
                    if let Err(cleanup) = self.ports.shares.delete(id).await {
                        tracing::error!(share = name.as_str(), error = %cleanup, "cannot forget a failed share");
                    }
                    if let Err(cleanup) = self.apply_locked(request).await {
                        tracing::warn!(share = name.as_str(), error = %cleanup, "file services not applied after a failed share");
                    }
                }
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
            Some((access, removed)) => Some(
                self.access_job(share.volume, share.name.clone(), access, removed, actor, request).await?,
            ),
            None => None,
        };
        Ok((share, job))
    }

    /// A job that gives everything in the share the permissions its
    /// directory got already.
    pub(super) async fn access_job(
        &self,
        volume: u16,
        name: ShareName,
        access: Vec<(Principal, Permission)>,
        removed: Vec<Principal>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Job, SharingError> {
        let host = Arc::clone(&self.ports.host);
        let request = request.clone();
        let label = name.to_string();
        Ok(self
            .ports
            .jobs
            .start("sharing.apply_access", Some(label), Some(actor), move |progress| async move {
                progress.step("existing_files", None).await;
                host.set_access(volume, &name, &access, &removed, true, &request)
                    .await
                    .map_err(|error| JobError::new("agent_failed", Some(error.to_string())))
            })
            .await?)
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
        // Offered no more before its data goes, but recorded until it has
        // gone: if applying fails, the share is back as it was; if its data
        // cannot go, that is tried again (finish_deletions).
        self.ports.shares.set_deleting(id, true).await?;
        if let Err(error) = self.apply_locked(request).await {
            self.ports.shares.set_deleting(id, false).await?;
            return Err(error);
        }

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
                    let mut outcome = this
                        .ports
                        .host
                        .destroy_dataset(&share.pool, share.volume, &share.name, &request_owned)
                        .await
                        .map_err(SharingError::from);
                    if outcome.is_ok() {
                        outcome = this.ports.shares.delete(share.id).await.map_err(SharingError::from);
                    }
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

    /// Destroys the data of shares whose deletion did not finish (their
    /// dataset was busy, or zendiloOS stopped in between) and forgets them;
    /// zendiloosd calls it when it starts. Says how many are still left.
    pub async fn finish_deletions(&self, request: &RequestContext) -> Result<usize, SharingError> {
        let _changes = self.changes.lock().await;
        let mut left = 0;
        for share in self.ports.shares.deleting().await? {
            match self.ports.host.destroy_dataset(&share.pool, share.volume, &share.name, request).await {
                Ok(()) => self.ports.shares.delete(share.id).await?,
                Err(error) => {
                    left += 1;
                    tracing::warn!(share = share.name.as_str(), %error, "the data of a deleted share is still there");
                }
            }
        }
        Ok(left)
    }
}
