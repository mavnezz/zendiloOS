//! The plugin, the servers, and tasks made, changed and removed.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext, Secret, Timestamp};

use super::ports::{HostFacts, HostKey, Location, NewTask, PasswordChange, Readers};
use super::{BackupError, Backups, audited};
use crate::domain::{
    Entry, Invalid, Mode, NasPlace, Place, RemotePlace, Run, Settings, Task, TaskSettings, folder_name,
    is_host,
};

/// The plugin as its page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub facts: HostFacts,
}

/// A task with how it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub task: Task,
    /// A daemon's password is stored (it is never shown).
    pub password_set: bool,
    pub last_run: Option<Run>,
    /// When its schedule runs it next.
    pub next_at: Option<Timestamp>,
    pub running: bool,
    /// Whether its places on the NAS are there now: the share, the drive
    /// plugged in. A server's is not looked at here.
    pub reachable: bool,
}

impl Backups {
    pub async fn plugin(&self, context: &RequestContext) -> Result<Plugin, BackupError> {
        Ok(Plugin { facts: self.ports.host.facts(context).await? })
    }

    /// Installs rsync and the SSH client from Debian, in a job.
    pub async fn install(&self, actor: &Actor, context: &RequestContext) -> Result<Job, BackupError> {
        if self.ports.host.facts(context).await?.rsync {
            return Err(BackupError::Installed);
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("backup.install", Some("rsync".into()), Some(actor), move |progress| async move {
                progress.step("packages", Some(0.05)).await;
                let done = this.ports.host.install(&context).await.map_err(BackupError::from);
                this.audit("backup.install", audited(&done), "rsync", Some(&who), &context).await;
                done.map_err(|error| super::job_error(&error))
            })
            .await?;
        Ok(job)
    }

    /// The NAS's public key for SSH servers: made the first time it is
    /// asked for. Its private key stays on the NAS, readable by root alone.
    pub async fn public_key(&self, actor: &Actor, context: &RequestContext) -> Result<String, BackupError> {
        self.require_installed(context).await?;
        if let Some(key) = self.ports.host.facts(context).await?.public_key {
            return Ok(key);
        }
        let made = self.ports.host.make_key(context).await;
        self.audit("backup.key", audited(&made), "ssh", Some(actor), context).await;
        Ok(made?)
    }

    /// The host keys an SSH server offers, for the administrator to compare
    /// and pin.
    pub async fn host_keys(
        &self,
        host: &str,
        port: u16,
        context: &RequestContext,
    ) -> Result<Vec<HostKey>, BackupError> {
        self.require_installed(context).await?;
        if !is_host(host) || port == 0 {
            return Err(Invalid("that is no server's address and port".into()).into());
        }
        let keys = self.ports.host.host_keys(host, port, context).await?;
        if keys.is_empty() {
            return Err(BackupError::Refused(format!("{host} offered no host key")));
        }
        Ok(keys)
    }

    /// Lists a folder of a server, to see that it can be reached as given:
    /// with `password`, or the one task `task` has stored for the same
    /// daemon.
    pub async fn check_server(
        &self,
        place: &RemotePlace,
        password: Option<Secret>,
        task: Option<i64>,
        context: &RequestContext,
    ) -> Result<Vec<Entry>, BackupError> {
        self.require_installed(context).await?;
        place.check()?;
        let password = match (password.filter(|p| !p.expose().is_empty()), task) {
            (Some(password), _) => Some(password),
            (None, Some(id)) => {
                let stored = self.find(id).await?;
                match stored.server() {
                    Some(server) if server.keeps_password_for(&place.server) => {
                        self.ports.tasks.password(id).await?
                    }
                    _ => None,
                }
            }
            (None, None) => None,
        };
        let at = Location::Remote { server: place.server.clone(), path: place.path.clone() };
        Ok(self.ports.host.list(&at, password.as_ref(), context).await?)
    }

    /// Who may read `share_id`: whoever may read a share reads the backups
    /// put into it, whatever the source's rules said.
    pub async fn readers(&self, share_id: i64) -> Result<Readers, BackupError> {
        self.ports.shares.readers(share_id).await?.ok_or(BackupError::NotFound)
    }

    pub async fn tasks(&self, context: &RequestContext) -> Result<Vec<TaskView>, BackupError> {
        let tasks = self.ports.tasks.list().await?;
        let drives = if tasks.iter().any(|task| task.target_drive().is_some()) {
            self.ports.drives.drives(context).await.unwrap_or_else(|error| {
                tracing::warn!(%error, "backup: cannot look at the removable drives");
                Vec::new()
            })
        } else {
            Vec::new()
        };
        let mut views = Vec::with_capacity(tasks.len());
        for task in tasks {
            views.push(self.view(task, &drives).await?);
        }
        Ok(views)
    }

    pub async fn task(&self, id: i64, context: &RequestContext) -> Result<TaskView, BackupError> {
        let task = self.find(id).await?;
        let drives =
            if task.target_drive().is_some() { self.ports.drives.drives(context).await? } else { Vec::new() };
        self.view(task, &drives).await
    }

    async fn view(&self, task: Task, drives: &[super::Drive]) -> Result<TaskView, BackupError> {
        let last_run = self.ports.runs.list(Some(task.id), 1).await?.into_iter().next();
        let now = self.now();
        let next_at = task.settings.schedule.map(|schedule| {
            let offsets = |at| self.ports.clock.utc_offset(at);
            // A time missed (off, or busy) runs at once.
            if schedule.is_due(task.since, now, &offsets) { now } else { schedule.next(now, &offsets) }
        });
        let mut reachable = true;
        for place in [&task.settings.source, &task.settings.target] {
            match place {
                Place::Nas(NasPlace::Share { share_id, .. }) => {
                    reachable &= self.ports.shares.locate(*share_id).await?.is_some();
                }
                Place::Nas(NasPlace::Drive { drive, .. }) => {
                    reachable &= drives.iter().any(|d| d.id == *drive && d.mounted);
                }
                Place::Remote(_) => {}
            }
        }
        let password_set = task.server().is_some_and(|s| s.is_daemon())
            && self.ports.tasks.password(task.id).await?.is_some();
        Ok(TaskView { running: self.running(task.id), task, password_set, last_run, next_at, reachable })
    }

    /// Checks what only the places can tell: the shares are there, a drive
    /// target can hold versions, and no other task writes into the folder.
    async fn check_places(
        &self,
        settings: &TaskSettings,
        folder: &str,
        except: Option<i64>,
        context: &RequestContext,
    ) -> Result<Option<String>, BackupError> {
        for place in [&settings.source, &settings.target] {
            if let Place::Nas(NasPlace::Share { share_id, .. }) = place
                && self.ports.shares.locate(*share_id).await?.is_none()
            {
                return Err(BackupError::ShareMissing);
            }
        }
        let others = self.ports.tasks.list().await?;
        if let Some(other) = others.iter().find(|other| {
            Some(other.id) != except && other.settings.target == settings.target && other.folder == folder
        }) {
            return Err(BackupError::FolderTaken(other.settings.name.clone()));
        }
        // A drive plugged in now says whether it can hold versions, and
        // which plugging in a task for it starts from.
        let Some(drive) = settings.target.nas().and_then(NasPlace::drive) else { return Ok(None) };
        let found = self.ports.drives.drives(context).await?.into_iter().find(|d| d.id == drive);
        if let Some(found) = &found
            && matches!(settings.mode, Mode::Versioned { .. })
            && !found.links()
        {
            return Err(Invalid(format!(
                "versions need hard links, which {} has not: back up as a mirror, or format the drive as ext4",
                found.filesystem
            ))
            .into());
        }
        Ok(found.filter(|d| d.mounted).and_then(|d| d.plugged_in))
    }

    async fn names(&self, except: Option<i64>) -> Result<Vec<String>, BackupError> {
        Ok(self
            .ports
            .tasks
            .list()
            .await?
            .into_iter()
            .filter(|task| Some(task.id) != except)
            .map(|task| task.settings.name)
            .collect())
    }

    /// Makes a task. A daemon's password is stored, never shown again.
    pub async fn create(
        &self,
        settings: TaskSettings,
        password: Option<Secret>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<TaskView, BackupError> {
        self.require_installed(context).await?;
        let names = self.names(None).await?;
        let settings = settings.check(&names.iter().map(String::as_str).collect::<Vec<_>>())?;
        let folder = folder_name(&settings.name);
        let last_plug = self.check_places(&settings, &folder, None, context).await?;
        let password = password.filter(|p| !p.expose().is_empty() && settings_daemon(&settings));
        let now = self.now();
        let new = NewTask { settings, folder, created_at: now, last_plug };
        let created = self.ports.tasks.create(&new, password.as_ref()).await;
        let done = match created {
            Ok(Some(id)) => Ok(id),
            Ok(None) => Err(BackupError::NameTaken),
            Err(error) => Err(error.into()),
        };
        self.audit("backup.task_create", audited(&done), &new.settings.name, Some(actor), context).await;
        self.task(done?, context).await
    }

    /// Changes a task. Its folder in the target stays as it was named. A
    /// daemon's password stays unless another is given, and only for the
    /// same daemon, port, user and module.
    pub async fn update(
        &self,
        id: i64,
        settings: TaskSettings,
        password: Option<Secret>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<TaskView, BackupError> {
        let task = self.find(id).await?;
        if self.running(id) {
            return Err(BackupError::Running);
        }
        let names = self.names(Some(id)).await?;
        let settings = settings.check(&names.iter().map(String::as_str).collect::<Vec<_>>())?;
        let mut last_plug = self.check_places(&settings, &task.folder, Some(id), context).await?;
        if settings.target == task.settings.target && last_plug.is_none() {
            last_plug = task.last_plug.clone();
        }
        let change = match password.filter(|p| !p.expose().is_empty()) {
            Some(password) if settings_daemon(&settings) => PasswordChange::Set(password),
            _ => match (task.server(), settings_server(&settings)) {
                (Some(old), Some(new)) if old.keeps_password_for(new) => PasswordChange::Keep,
                _ => PasswordChange::Clear,
            },
        };
        let changed = Task { settings, since: self.now(), last_plug, ..task };
        let done = match self.ports.tasks.update(&changed, change).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(BackupError::NameTaken),
            Err(error) => Err(error.into()),
        };
        self.audit("backup.task_change", audited(&done), &changed.settings.name, Some(actor), context).await;
        done?;
        self.task(id, context).await
    }

    /// Forgets a task and its log; what it backed up stays where it is.
    pub async fn delete(&self, id: i64, actor: &Actor, context: &RequestContext) -> Result<(), BackupError> {
        let task = self.find(id).await?;
        let _claim = self.claim(id, false)?;
        let done = self.ports.tasks.delete(id).await;
        self.audit("backup.task_delete", audited(&done), task.name(), Some(actor), context).await;
        done?;
        Ok(())
    }

    pub async fn settings(&self) -> Result<Settings, BackupError> {
        Ok(self.ports.settings.get().await?)
    }

    pub async fn set_settings(
        &self,
        settings: Settings,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Settings, BackupError> {
        let settings = settings.check().map_err(|e| Invalid(e.to_string()))?;
        let done = self.ports.settings.set(settings).await;
        self.audit("backup.settings", audited(&done), "settings", Some(actor), context).await;
        done?;
        Ok(settings)
    }
}

fn settings_server(settings: &TaskSettings) -> Option<&crate::domain::Server> {
    settings.source.remote().or(settings.target.remote()).map(|remote| &remote.server)
}

fn settings_daemon(settings: &TaskSettings) -> bool {
    settings_server(settings).is_some_and(crate::domain::Server::is_daemon)
}
