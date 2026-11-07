//! What this context needs from the outside world.

use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_kernel::{Actor, PortError, RequestContext};

use crate::domain::{
    ComposeListing, ComposeModel, Container, ContainerAction, DiskUsage, Image, Network, OwnProject,
    ProjectName, RegistryImage, UpdateSettings, UsageSample,
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

    /// Deploys an administrator's project from `config`, what Compose made
    /// of the file they confirmed; with `pull`, its images are fetched
    /// first.
    async fn deploy_project(
        &self,
        project: &str,
        config: &str,
        pull: bool,
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
