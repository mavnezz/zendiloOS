//! What this context needs from the outside world.

use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::{Actor, PortError, RequestContext};

use crate::domain::{
    Backup, BackupFile, BackupPlan, ComposeListing, ComposeModel, Container, ContainerAction,
    ContainerDetails, ContainerProcess, DiskUsage, Image, Network, OwedStart, OwnProject, ProjectName,
    RegistryImage, UpdateSettings, UsageSample, Volume, VolumeSize, WrittenBackup,
};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Docker or the agent refused, in their words: a Compose file it
    /// cannot read, a running container to remove, an image in use.
    #[error("{0}")]
    Refused(String),
    /// Docker did not do what was asked, in its words.
    #[error("{0}")]
    Failed(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Where the lines Docker writes while it deploys go, for a person to
/// follow: the job's output.
pub trait Lines: Send + Sync {
    fn line(&self, line: &str);
}

/// Lines nobody follows.
pub struct NoLines;

impl Lines for NoLines {
    fn line(&self, _: &str) {}
}

/// What a project's Compose action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAction {
    Start,
    Stop,
    Restart,
    /// Creates and starts the containers, recreating changed ones.
    Up,
    Pull,
    /// Removes the containers and networks; images and data stay.
    Down,
}

impl ProjectAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Up => "up",
            Self::Pull => "pull",
            Self::Down => "down",
        }
    }
}

/// What is typed into a terminal, or its new size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    Data(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

/// A terminal open in a container: what its program writes comes on
/// `output`, which closes once the program ended; what goes to `input` is
/// typed. Dropping `input` hangs up, which ends the program.
#[derive(Debug)]
pub struct TerminalSession {
    pub output: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub input: tokio::sync::mpsc::Sender<TerminalInput>,
}

/// The Docker engine, through the privileged agent.
#[async_trait]
pub trait DockerEngine: Send + Sync {
    async fn containers(&self, context: &RequestContext) -> Result<Vec<Container>, EngineError>;

    /// One sample of every container's use; takes a moment.
    async fn usage(&self, context: &RequestContext) -> Result<UsageSample, EngineError>;

    async fn images(&self, context: &RequestContext) -> Result<Vec<Image>, EngineError>;

    async fn networks(&self, context: &RequestContext) -> Result<Vec<Network>, EngineError>;

    async fn disk_usage(&self, context: &RequestContext) -> Result<DiskUsage, EngineError>;

    async fn compose_projects(&self, context: &RequestContext) -> Result<Vec<ComposeListing>, EngineError>;

    async fn control_container(
        &self,
        id: &str,
        action: ContainerAction,
        context: &RequestContext,
    ) -> Result<(), EngineError>;

    async fn container_logs(
        &self,
        id: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, EngineError>;

    /// One container as Docker inspects it.
    async fn inspect_container(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<ContainerDetails, EngineError>;

    /// The processes of a running container.
    async fn container_processes(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Vec<ContainerProcess>, EngineError>;

    /// Starts `command` in a running container on a terminal of
    /// `columns` × `rows`.
    async fn open_terminal(
        &self,
        id: &str,
        command: &[String],
        columns: u16,
        rows: u16,
        context: &RequestContext,
    ) -> Result<TerminalSession, EngineError>;

    /// A project's configuration as Compose normalized it when it was
    /// confirmed, read: its services and what they bind where.
    fn read_config(&self, config: &str) -> Result<ComposeModel, EngineError>;

    async fn remove_image(&self, id: &str, context: &RequestContext) -> Result<(), EngineError>;

    async fn pull_image(&self, reference: &str, context: &RequestContext) -> Result<(), EngineError>;

    /// Removes the images no container uses; says what was freed.
    async fn prune_images(&self, context: &RequestContext) -> Result<String, EngineError>;

    async fn create_network(
        &self,
        name: &str,
        subnet: Option<&str>,
        gateway: Option<&str>,
        context: &RequestContext,
    ) -> Result<(), EngineError>;

    async fn remove_network(&self, name: &str, context: &RequestContext) -> Result<(), EngineError>;

    async fn volumes(&self, context: &RequestContext) -> Result<Vec<Volume>, EngineError>;

    /// What the local volumes' data takes up; takes a while.
    async fn volume_sizes(&self, context: &RequestContext) -> Result<Vec<VolumeSize>, EngineError>;

    /// Docker refuses one a container uses, also a stopped one.
    async fn remove_volume(&self, name: &str, context: &RequestContext) -> Result<(), EngineError>;

    /// Removes the volumes no container uses, named ones too; says what was
    /// freed.
    async fn prune_volumes(&self, context: &RequestContext) -> Result<String, EngineError>;

    /// A local volume with `labels`.
    async fn create_volume(
        &self,
        name: &str,
        labels: &BTreeMap<String, String>,
        context: &RequestContext,
    ) -> Result<(), EngineError>;

    /// `compose` as Compose reads it with `directory` as its folder (a
    /// `.env` there does not count); [`EngineError::Refused`] with
    /// Compose's words if it cannot.
    async fn check_compose(
        &self,
        project: &ProjectName,
        directory: &str,
        compose: &str,
        context: &RequestContext,
    ) -> Result<CheckedCompose, EngineError>;

    /// Acts on a project not made here; `Down` also on an administrator's,
    /// with what deploying it kept.
    async fn control_project(
        &self,
        project: &str,
        action: ProjectAction,
        context: &RequestContext,
    ) -> Result<(), EngineError>;

    /// As [`DockerEngine::control_project`], telling `lines` what Docker
    /// writes as it comes (pulling, bringing up).
    async fn control_project_reporting(
        &self,
        project: &str,
        action: ProjectAction,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let _ = lines;
        self.control_project(project, action, context).await
    }

    /// Deploys an administrator's project from `config`, what Compose made
    /// of the file they confirmed; with `pull`, its images are fetched
    /// first. What Compose writes goes to `lines` as it comes.
    async fn deploy_project(
        &self,
        project: &str,
        config: &str,
        pull: bool,
        lines: &dyn Lines,
        context: &RequestContext,
    ) -> Result<(), EngineError>;

    async fn project_logs(
        &self,
        project: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, EngineError>;

    /// The Compose files of a project not made here, by path.
    async fn project_files(
        &self,
        project: &str,
        context: &RequestContext,
    ) -> Result<BTreeMap<String, String>, EngineError>;
}

/// The projects administrators made here.
#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// By name.
    async fn list(&self) -> Result<Vec<OwnProject>, PortError>;

    /// `false` if the name is taken.
    async fn create(&self, project: &OwnProject) -> Result<bool, PortError>;

    /// Keeps `compose` as confirmed and `config` as what runs; `false` if
    /// there is no such project.
    async fn confirm(&self, name: &ProjectName, compose: &str, config: &str) -> Result<bool, PortError>;

    /// `false` if there was none.
    async fn delete(&self, name: &ProjectName) -> Result<bool, PortError>;
}

/// A Compose file as Compose read it: what it lets containers do, and the
/// whole project normalized, as JSON, which is what runs.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedCompose {
    pub model: ComposeModel,
    pub config: String,
}

/// A share an administrator may put a project's folder in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlace {
    pub share_id: i64,
    pub share: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("the share does not exist or cannot be written")]
    Denied,
    #[error("there is no such file")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Projects' folders and Compose files in shares, written and read with
/// the permissions of the administrator who acts.
#[async_trait]
pub trait ProjectStorage: Send + Sync {
    /// The shares `actor` may write.
    async fn places(&self, actor: &Actor) -> Result<Vec<ProjectPlace>, StorageError>;

    /// Where `folder` of the share is on the machine: `/volume1/docker/x`.
    async fn directory(&self, actor: &Actor, share_id: i64, folder: &str) -> Result<String, StorageError>;

    /// Writes `content` as `name` into `folder` of the share, making the
    /// folder first, replacing a file of that name.
    async fn write(
        &self,
        actor: &Actor,
        share_id: i64,
        folder: &str,
        name: &str,
        content: &str,
        context: &RequestContext,
    ) -> Result<(), StorageError>;

    async fn read(
        &self,
        actor: &Actor,
        share_id: i64,
        path: &str,
        context: &RequestContext,
    ) -> Result<String, StorageError>;

    /// Makes the folder `path` of the share and each one on the way, as far
    /// as they are missing; what is there already stays as it is.
    async fn make_folder(
        &self,
        actor: &Actor,
        share_id: i64,
        path: &str,
        context: &RequestContext,
    ) -> Result<(), StorageError>;
}

/// What registries publish.
#[async_trait]
pub trait Registries: Send + Sync {
    /// The digest under `image`'s tag now; why it cannot be told otherwise.
    async fn digest(&self, image: &RegistryImage) -> Result<String, String>;
}

/// When images are checked and taken; the defaults until set.
#[async_trait]
pub trait UpdateSettingsRepository: Send + Sync {
    async fn settings(&self) -> Result<UpdateSettings, PortError>;

    async fn save(&self, settings: &UpdateSettings) -> Result<(), PortError>;
}

/// A pool Docker's data is on or could go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPlace {
    pub pool: String,
    pub volume: u16,
    /// Free for files, if known.
    pub available_bytes: Option<u64>,
}

/// Where Docker keeps its data, through the privileged agent.
#[async_trait]
pub trait DockerData: Send + Sync {
    /// The pool it is on, if one zendiloOS made, and the pools it could go
    /// to.
    async fn places(
        &self,
        context: &RequestContext,
    ) -> Result<(Option<DataPlace>, Vec<DataPlace>), PortError>;

    /// Stops Docker, copies its data and starts it from the copy; long.
    async fn move_to(
        &self,
        from: &DataPlace,
        to: &DataPlace,
        context: &RequestContext,
    ) -> Result<(), EngineError>;
}

/// Where a share is on the machine: `/volume<volume>/<share>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharePlace {
    pub volume: u16,
    pub share: String,
}

/// The shares backups go to, by their ID: backups run on a schedule, with
/// nobody signed in.
#[async_trait]
pub trait BackupShares: Send + Sync {
    /// `None` once the share is gone.
    async fn locate(&self, share_id: i64) -> Result<Option<SharePlace>, PortError>;
}

/// Where a long step reports how far it is, in percent, and what is worth
/// a line of its own.
pub trait Steps: Send + Sync {
    fn step(&self, step: &str, percent: Option<u8>);

    fn note(&self, line: &str) {
        let _ = line;
    }
}

/// Steps nobody follows.
pub struct NoSteps;

impl Steps for NoSteps {
    fn step(&self, _: &str, _: Option<u8>) {}
}

/// Volumes backed up into shares and restored from them, through the
/// privileged agent; each can take hours. The agent answers once the work
/// ended; when no answer came, [`VolumeArchiver::end_work`] makes sure.
#[async_trait]
pub trait VolumeArchiver: Send + Sync {
    /// Writes `<folder>/<volume>/<stem>.tar.zst` and `<stem>.json` into the
    /// share.
    async fn back_up(
        &self,
        volume: &str,
        place: &SharePlace,
        folder: &str,
        stem: &str,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<WrittenBackup, EngineError>;

    /// Replaces the volume's contents with the archive `written` names in
    /// the share, if it is still the one written: its SHA-256, no more data
    /// and entries than it held. The folder beside the data the old
    /// contents are partly left in, if they could not all be removed.
    async fn restore(
        &self,
        volume: &str,
        place: &SharePlace,
        written: &WrittenBackup,
        steps: &dyn Steps,
        context: &RequestContext,
    ) -> Result<Option<String>, EngineError>;

    /// Removes a file a backup wrote if it still is that file; whether it
    /// did.
    async fn remove(
        &self,
        place: &SharePlace,
        file: &BackupFile,
        context: &RequestContext,
    ) -> Result<bool, EngineError>;

    /// Stops what a backup or restore of `volume` may have left running and
    /// answers once nothing works on it any more: whether something still
    /// did.
    async fn end_work(&self, volume: &str, context: &RequestContext) -> Result<bool, EngineError>;
}

/// A run of a backup as it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBackup {
    pub volume: String,
    pub share_id: i64,
    pub started_at: zendiloos_kernel::Timestamp,
    pub by: Option<String>,
}

/// The volumes' backup plans and the runs of backups.
#[async_trait]
pub trait BackupRepository: Send + Sync {
    /// By volume.
    async fn plans(&self) -> Result<Vec<BackupPlan>, PortError>;

    /// Makes or replaces the volume's plan.
    async fn save_plan(&self, plan: &BackupPlan) -> Result<(), PortError>;

    /// `false` if there was none.
    async fn delete_plan(&self, volume: &str) -> Result<bool, PortError>;

    /// Runs of the plan are due from `at` on.
    async fn ran(&self, volume: &str, at: zendiloos_kernel::Timestamp) -> Result<(), PortError>;

    async fn start(&self, backup: &NewBackup) -> Result<i64, PortError>;

    /// How the run ended: succeeded with what it wrote, or failed.
    async fn finish(
        &self,
        id: i64,
        at: zendiloos_kernel::Timestamp,
        outcome: Result<&WrittenBackup, &str>,
    ) -> Result<(), PortError>;

    /// The runs, of one volume or all, the newest first.
    async fn backups(&self, volume: Option<&str>) -> Result<Vec<Backup>, PortError>;

    async fn backup(&self, id: i64) -> Result<Option<Backup>, PortError>;

    async fn forget(&self, id: i64) -> Result<(), PortError>;

    /// Runs still running from before a restart, now failed; as they were.
    async fn interrupted(&self, at: zendiloos_kernel::Timestamp) -> Result<Vec<Backup>, PortError>;

    /// The containers backups and restores stopped and nothing started
    /// since, with the volume each was stopped for.
    async fn owed_starts(&self) -> Result<Vec<OwedStart>, PortError>;

    /// Notes `container`, stopped for `volume`, before it is stopped.
    async fn owe_start(
        &self,
        container: &str,
        volume: &str,
        at: zendiloos_kernel::Timestamp,
    ) -> Result<(), PortError>;

    /// `container` started again, or is gone: owed nothing any more.
    async fn started(&self, container: &str) -> Result<(), PortError>;
}
