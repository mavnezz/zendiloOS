//! Use cases on shares. Every change keeps the database and the SMB and
//! NFS servers' configuration in step; the configuration is always written
//! as a whole from the database. Access is the share's as a whole
//! (docs/architecture.md, 4.4): a change of rules changes no file, only
//! what Samba and the file manager allow, and a change that takes access
//! away from anyone also ends the share's open SMB connections.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use zendiloos_jobs::{Job, JobError};
use zendiloos_kernel::{Actor, AuditEntry, Outcome, PortError, RequestContext};

use super::SharingPorts;
use super::migrations::Migrations;
use super::ports::{HostError, Names};
use crate::domain::{
    Access, AccessRule, FileServices, HomesSpec, InvalidFileServices, InvalidNfsHost, InvalidShareName,
    NewShare, NfsClient, NfsSpec, Permission, Principal, Share, ShareId, ShareName, ShareSettings,
    ShareUsage, SharingConfig, SmbSpec, Subject, effective, nfs_host,
};

pub const MAX_DESCRIPTION_CHARS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SharingError {
    #[error(transparent)]
    InvalidName(#[from] InvalidShareName),
    #[error(transparent)]
    InvalidNetwork(#[from] InvalidNfsHost),
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
    /// Administrators' projects live in the share or mount something of
    /// it; they are named.
    #[error("projects use this share: {0}")]
    UsedByProjects(String),
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
    /// Offered on the network at once.
    pub shared: bool,
    pub recycle_bin: bool,
    pub access: Vec<AccessRule>,
    /// As given: each client is checked and normalized ([`nfs_host`]).
    pub nfs: Vec<NfsClient>,
    pub settings: ShareSettings,
}

/// Changes to a share; `None` leaves a field as it is.
#[derive(Default)]
pub struct ShareChanges {
    pub description: Option<Option<String>>,
    /// On or off the network; folder, data and rules stay.
    pub shared: Option<bool>,
    pub recycle_bin: Option<bool>,
    pub access: Option<Vec<AccessRule>>,
    pub nfs: Option<Vec<NfsClient>>,
    pub settings: Option<ShareSettings>,
}

fn description(value: Option<&str>) -> Result<Option<String>, SharingError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if value.chars().count() > MAX_DESCRIPTION_CHARS || value.chars().any(char::is_control) {
        return Err(SharingError::InvalidDescription);
    }
    Ok(Some(value.to_owned()))
}

fn clients(input: Vec<NfsClient>) -> Result<Vec<NfsClient>, SharingError> {
    let mut clients: Vec<NfsClient> = Vec::new();
    for client in input {
        let normalized = nfs_host(&client.network)?;
        if clients.iter().any(|c| c.network == normalized) {
            return Err(SharingError::Duplicate);
        }
        clients.push(NfsClient { network: normalized, ..client });
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

/// The groups `account` is in, as `names` say.
pub(super) fn groups_of(names: &Names, account: i64) -> Vec<i64> {
    let mut groups: Vec<i64> = names
        .members
        .iter()
        .filter(|(_, members)| members.contains(&account))
        .map(|(group, _)| *group)
        .collect();
    groups.sort_unstable();
    groups
}

/// The rules as Samba enforces them, by name, groups as groups: Samba
/// follows their members itself. A disabled account may do nothing: its
/// own rule becomes a denial, and where a group's rule would let it in, it
/// is denied by name (`invalid users` wins over the group in `valid
/// users`). Rules of accounts or groups deleted since are left out.
pub(super) fn smb_access(rules: &[AccessRule], names: &Names) -> Vec<(Principal, Permission)> {
    let mut access: Vec<(Principal, Permission)> = rules
        .iter()
        .filter_map(|rule| match rule.subject {
            Subject::Account(id) => {
                let permission =
                    if names.disabled.contains(&id) { Permission::Deny } else { rule.permission };
                Some((Principal::User(names.accounts.get(&id)?.clone()), permission))
            }
            Subject::Group(id) => Some((Principal::Group(names.groups.get(&id)?.clone()), rule.permission)),
        })
        .collect();
    let mut disabled: Vec<i64> = names.disabled.iter().copied().collect();
    disabled.sort_unstable();
    for id in disabled {
        let Some(name) = names.accounts.get(&id) else { continue };
        if rules.iter().any(|rule| rule.subject == Subject::Account(id)) {
            continue;
        }
        let groups = groups_of(names, id);
        let let_in = rules.iter().any(|rule| {
            matches!(rule.subject, Subject::Group(group) if groups.contains(&group))
                && rule.permission != Permission::Deny
        });
        if let_in {
            access.push((Principal::User(name.clone()), Permission::Deny));
        }
    }
    access
}

/// What each account may do on `share` over SMB, by ID: nothing for a
/// disabled one, nothing at all while the share is not on the network.
pub(super) fn reach(share: &Share, names: &Names) -> BTreeMap<i64, Access> {
    if !share.shared {
        return BTreeMap::new();
    }
    names
        .accounts
        .keys()
        .filter(|id| !names.disabled.contains(id))
        .filter_map(|id| Some((*id, effective(&share.access, *id, &groups_of(names, *id))?)))
        .collect()
}

/// Whether going from `before` to `after` takes anything away from
/// anyone: access altogether, or writing.
pub(super) fn narrowed(before: &BTreeMap<i64, Access>, after: &BTreeMap<i64, Access>) -> bool {
    before.iter().any(|(id, was)| match after.get(id) {
        None => true,
        Some(now) => *was == Access::ReadWrite && *now == Access::Read,
    })
}

/// What the servers should offer for `shares`: those shared on the
/// network; the others are not offered at all.
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
            .filter(|share| share.shared)
            .map(|share| SmbSpec {
                name: share.name.clone(),
                volume: share.volume,
                access: smb_access(&share.access, names),
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
            .filter(|share| share.shared && !share.nfs.is_empty())
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
    pub(super) migrations: Arc<Migrations>,
}

/// Who may read a share, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareReaders {
    pub accounts: Vec<String>,
    pub groups: Vec<String>,
    /// Hosts and networks it is exported to over NFS.
    pub nfs: Vec<String>,
}

impl SharingUseCases {
    pub fn new(ports: SharingPorts) -> Self {
        Self {
            ports,
            changes: Arc::new(tokio::sync::Mutex::new(())),
            snapshot_failures: Arc::default(),
            migrations: Arc::default(),
        }
    }

    pub async fn list(&self) -> Result<Vec<Share>, SharingError> {
        Ok(self.ports.shares.list().await?)
    }

    /// Who may read share `id`, by name: the accounts and groups its rules
    /// let read (a disabled account may not), and the NFS clients it is
    /// exported to while it is shared. `None` if there is no such share.
    pub async fn readers(&self, id: i64) -> Result<Option<ShareReaders>, SharingError> {
        let Some(share) = self.ports.shares.list().await?.into_iter().find(|share| share.id.get() == id)
        else {
            return Ok(None);
        };
        let names = self.ports.directory.names().await?;
        let mut readers = ShareReaders::default();
        for rule in share.access.iter().filter(|rule| rule.permission.grant().is_some()) {
            match rule.subject {
                Subject::Account(account) if !names.disabled.contains(&account) => {
                    readers.accounts.extend(names.accounts.get(&account).cloned());
                }
                Subject::Group(group) => readers.groups.extend(names.groups.get(&group).cloned()),
                Subject::Account(_) => {}
            }
        }
        if share.shared {
            readers.nfs = share.nfs.iter().map(|client| client.network.clone()).collect();
        }
        readers.accounts.sort();
        readers.groups.sort();
        Ok(Some(readers))
    }

    /// Writes the servers' configuration from the database, e.g. at startup.
    pub async fn apply(&self, request: &RequestContext) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        self.apply_locked(&[], request).await?;
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

    /// Writes the servers' configuration from the database; then the open
    /// SMB connections of `close` end. Called with `changes` held.
    pub(super) async fn apply_locked(
        &self,
        close: &[ShareName],
        request: &RequestContext,
    ) -> Result<(), SharingError> {
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
        let config = config(&shares, &names, services, homes);
        self.ports.host.apply(&config, close, request).await?;
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
        if let Some(machine) = services.time_machine {
            let share = self.ports.shares.find(machine.share).await?.ok_or(SharingError::ShareNotFound)?;
            if !share.shared {
                return Err(SharingError::Refused(format!(
                    "{} is not shared on the network; Macs cannot back up to it",
                    share.name.as_str()
                )));
            }
        }
        let previous = self.ports.shares.file_services().await?;
        self.ports.shares.set_file_services(&services).await?;
        let applied = self.apply_locked(&[], request).await;
        if applied.is_err() {
            self.ports.shares.set_file_services(&previous).await?;
            if let Err(error) = self.apply_locked(&[], request).await {
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

    /// Creates the dataset, gives its folder what every share's has, stores
    /// the share and offers it. A failure after the dataset exists removes
    /// it again.
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
            // Root's and the group zendiloos-files', whatever the rules: they
            // are enforced where the share is used, never in its files.
            self.ports.host.prepare_share(volume, &name, request).await?;
            let new = NewShare {
                name: name.clone(),
                description,
                pool: input.pool.clone(),
                volume,
                shared: input.shared,
                recycle_bin: input.recycle_bin,
                settings: input.settings,
                access: input.access,
                nfs,
                created_at: self.ports.clock.now(),
            };
            let share = self.ports.shares.create(new).await?.ok_or(SharingError::NameTaken)?;
            recorded = Some(share.id);
            self.apply_locked(&[], request).await?;
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
                    if let Err(cleanup) = self.apply_locked(&[], request).await {
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

    /// Changes a share. New rules hold at once: Samba reads them again, and
    /// where they take access away from anyone (or the share goes off the
    /// network), the share's open SMB connections end, so that nobody keeps
    /// what they may no longer do. Its files stay as they are.
    pub async fn update(
        &self,
        id: ShareId,
        changes: ShareChanges,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Share, SharingError> {
        let description = changes.description.map(|d| description(d.as_deref())).transpose()?;
        let nfs = changes.nfs.map(clients).transpose()?;

        let _changes = self.changes.lock().await;
        let mut share = self.ports.shares.find(id).await?.ok_or(SharingError::ShareNotFound)?;
        let names = self.ports.directory.names().await?;
        let before = reach(&share, &names);
        // Everything is checked before anything changes.
        if let Some(access) = &changes.access {
            check_rules(access, &names)?;
        }
        if changes.shared == Some(false) && share.shared {
            let services = self.ports.shares.file_services().await?;
            if services.time_machine.is_some_and(|machine| machine.share == share.id) {
                return Err(SharingError::Refused(format!(
                    "Macs back up to {}; choose another share for Time Machine first",
                    share.name.as_str()
                )));
            }
        }
        let mut settings_change = None;
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
            settings_change = Some(settings);
        }
        // The host first, then the records.
        if let Some(settings) = settings_change {
            self.ports
                .host
                .set_properties(&share.pool, share.volume, &share.name, &settings, request)
                .await?;
            share.settings = settings;
        }
        if let Some(access) = changes.access {
            share.access = access;
        }
        if let Some(description) = description {
            share.description = description;
        }
        if let Some(recycle_bin) = changes.recycle_bin {
            share.recycle_bin = recycle_bin;
        }
        if let Some(shared) = changes.shared {
            share.shared = shared;
        }
        if let Some(nfs) = nfs {
            share.nfs = nfs;
        }
        share.updated_at = self.ports.clock.now();
        self.ports.shares.update(&share).await?;
        let close =
            if narrowed(&before, &reach(&share, &names)) { vec![share.name.clone()] } else { Vec::new() };
        self.apply_locked(&close, request).await?;
        self.audit("sharing.share_update", Outcome::Success, share.name.as_str(), actor, request).await;
        Ok(share)
    }

    /// Records what an app installed the earlier way, by its `account`, may
    /// do: exactly `grants` (share by name). Its entries on the shares'
    /// files stay as they are; the record keeps a share it uses from being
    /// deleted. The App Center gives an app nothing more any more, only
    /// takes it all when the app goes.
    pub async fn set_app_access(
        &self,
        account: &str,
        grants: &[(ShareName, Access)],
        _request: &RequestContext,
    ) -> Result<(), SharingError> {
        let _changes = self.changes.lock().await;
        let shares = self.ports.shares.list().await?;
        let mut wanted = Vec::new();
        for (name, access) in grants {
            let share = shares.iter().find(|s| &s.name == name).ok_or(SharingError::ShareNotFound)?;
            wanted.push((share.id, *access));
        }
        self.ports.shares.set_app_access(account, &wanted).await?;
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
        self.not_migrating(&share)?;
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
        let path = format!("/volume{}/{}", share.volume, share.name.as_str());
        let projects = self.ports.projects.using(share.id, &path).await?;
        if !projects.is_empty() {
            return Err(SharingError::UsedByProjects(projects.join(", ")));
        }
        // Offered no more before its data goes, but recorded until it has
        // gone: if applying fails, the share is back as it was; if its data
        // cannot go, that is tried again (finish_deletions). Its open
        // connections end.
        self.ports.shares.set_deleting(id, true).await?;
        if let Err(error) = self.apply_locked(std::slice::from_ref(&share.name), request).await {
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
                    let destroyed =
                        this.ports.host.destroy_dataset(&share.pool, share.volume, &share.name, &request_owned).await;
                    // Refused before anything went (something still uses it):
                    // the share is offered again, not left to be deleted later.
                    if let Err(HostError::Refused(reason)) = &destroyed {
                        let _changes = this.changes.lock().await;
                        let back = async {
                            this.ports.shares.set_deleting(share.id, false).await?;
                            this.apply_locked(&[], &request_owned).await
                        };
                        if let Err(error) = back.await {
                            tracing::error!(share = share.name.as_str(), %error, reason, "a share whose deletion was refused is not offered again");
                        }
                    }
                    let mut outcome = destroyed.map_err(SharingError::from);
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
