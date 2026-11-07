//! One container: shown in detail, its processes listed, a terminal opened
//! in it, its image updated.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, Outcome, RequestContext};

use super::ports::TerminalSession;
use super::projects::Deployment;
use super::{Containers, ContainersError, job_error};
use crate::domain::{
    Container, ContainerDetails, ContainerProcess, Origin, ProjectName, command_words, host_paths,
};

/// Terminals one account may have open at once.
pub const TERMINALS_PER_ACCOUNT: usize = 4;

/// A container in detail, with where each mount's files are as the
/// administrator named them ([`host_paths`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerDetailsView {
    pub details: ContainerDetails,
    /// One per mount of the container, in its order.
    pub host_paths: Vec<String>,
}

/// The terminals each account has open.
#[derive(Debug, Default)]
pub(crate) struct OpenTerminals(Mutex<BTreeMap<i64, usize>>);

/// A place among an account's terminals, given back when dropped.
#[derive(Debug)]
struct Seat {
    open: Arc<OpenTerminals>,
    account: i64,
}

impl Drop for Seat {
    fn drop(&mut self) {
        let mut open = self.open.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.account) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.account);
            }
        }
    }
}

/// A terminal open in a container, for one account.
#[derive(Debug)]
pub struct Terminal {
    pub session: TerminalSession,
    pub container: String,
    /// What it runs, as typed.
    pub command: String,
    _seat: Seat,
}

/// How a container is updated: what the dialog says before it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateWay {
    /// Its project is pulled and brought up again, the container made anew.
    Project(String),
    /// Outside a project: the newer image is only fetched; whoever made the
    /// container makes it anew.
    PullOnly(String),
}

impl Containers {
    /// `id` as Docker inspects it now; mounts of a project made here show
    /// the folders its confirmed file names, not the pins Docker sees.
    pub async fn container_details(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<ContainerDetailsView, ContainersError> {
        let container = self.find_container(id, context).await?;
        let details = self.ports.engine.inspect_container(&container.id, context).await?;
        let service = self.confirmed_service(&details.container).await?;
        let host_paths = host_paths(&details.container.mounts, service.as_ref());
        Ok(ContainerDetailsView { details, host_paths })
    }

    /// The service of `container` as its project was confirmed, for a
    /// project made here.
    async fn confirmed_service(
        &self,
        container: &Container,
    ) -> Result<Option<crate::domain::ComposeService>, ContainersError> {
        let (Some(project), Some(service)) = (&container.project, &container.service) else {
            return Ok(None);
        };
        let own = self.ports.projects.list().await?;
        let Some(config) = own.iter().find(|p| p.name.as_str() == project).and_then(|p| p.config.as_ref())
        else {
            return Ok(None);
        };
        Ok(self
            .ports
            .engine
            .read_config(config)
            .ok()
            .and_then(|model| model.services.into_iter().find(|candidate| &candidate.name == service)))
    }

    pub async fn container_processes(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Vec<ContainerProcess>, ContainersError> {
        let container = self.find_container(id, context).await?;
        if !container.state.is_up() {
            return Err(ContainersError::Refused(format!("{} is not running", container.name)));
        }
        Ok(self.ports.engine.container_processes(&container.id, context).await?)
    }

    /// Opens a terminal in a running container that runs `command` (words
    /// as typed): at most [`TERMINALS_PER_ACCOUNT`] per account, each
    /// recorded in the audit log. Any container: a terminal changes nothing
    /// of how the container was made, and administrators are trusted with
    /// the machine (an app installed the earlier way included).
    pub async fn open_terminal(
        &self,
        id: &str,
        command: &str,
        columns: u16,
        rows: u16,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Terminal, ContainersError> {
        let words = command_words(command).ok_or_else(|| {
            ContainersError::Invalid(format!(
                "{command:?} is no command such as /bin/sh: up to 16 words, the first no option"
            ))
        })?;
        let container = self.find_container(id, context).await?;
        if !container.state.is_up() {
            return Err(ContainersError::Refused(format!("{} is not running", container.name)));
        }
        let seat = self.take_seat(actor.account_id)?;
        let line = words.join(" ");
        let target = format!("{}: {line}", container.name);
        let opened = self
            .ports
            .engine
            .open_terminal(&container.id, &words, columns, rows, context)
            .await
            .map_err(ContainersError::from);
        let outcome = if opened.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("containers.container_terminal", outcome, &target, actor, context).await;
        Ok(Terminal { session: opened?, container: container.name, command: line, _seat: seat })
    }

    fn take_seat(&self, account: i64) -> Result<Seat, ContainersError> {
        let mut open = self.terminals.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = open.entry(account).or_insert(0);
        if *count >= TERMINALS_PER_ACCOUNT {
            return Err(ContainersError::Refused(format!(
                "{TERMINALS_PER_ACCOUNT} terminals are open already; end one first"
            )));
        }
        *count += 1;
        Ok(Seat { open: Arc::clone(&self.terminals), account })
    }

    /// How [`Containers::update_container`] would update `id`.
    pub async fn update_way(&self, id: &str, context: &RequestContext) -> Result<UpdateWay, ContainersError> {
        let container = self.find_container(id, context).await?;
        Ok(self.way_of(&container, context).await?.0)
    }

    async fn way_of(
        &self,
        container: &Container,
        context: &RequestContext,
    ) -> Result<(UpdateWay, Option<Deployment>), ContainersError> {
        let Some(name) = &container.project else {
            return Ok((UpdateWay::PullOnly(container.image.clone()), None));
        };
        let projects = self.projects(context).await?;
        let project = projects.into_iter().find(|p| &p.name == name).ok_or(ContainersError::NotFound)?;
        let deployment = match project.origin {
            Origin::Own(own) => Deployment::of(&own),
            Origin::Foreign { .. } => Deployment::Foreign,
            // Installed the earlier way: the App Center no longer updates it;
            // removed and installed again, it becomes a project of one's own.
            Origin::App { .. } => return Err(ContainersError::AppProject),
        };
        Ok((UpdateWay::Project(project.name), Some(deployment)))
    }

    /// Takes the newest image under the tag `id` runs, in a job: a
    /// container of a project brings its project up again with its images
    /// pulled, as the scheduled update does (a project made here from what
    /// was confirmed); one outside a project only gets the image fetched.
    pub async fn update_container(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let container = self.find_container(id, context).await?;
        let (way, deployment) = self.way_of(&container, context).await?;
        if let UpdateWay::Project(project) = &way {
            // Refused before a job starts, not inside it.
            ProjectName::parse(project)?;
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let target = container.name.clone();
        let job = self
            .ports
            .jobs
            .start("containers.update", Some(target.clone()), Some(actor), move |progress| async move {
                let done = match (&way, &deployment) {
                    (UpdateWay::Project(project), Some(how)) => {
                        progress.step("redeploying", None).await;
                        how.run(&this, project, true, &context).await.map_err(ContainersError::from)
                    }
                    (UpdateWay::PullOnly(image), _) | (UpdateWay::Project(image), None) => {
                        progress.step("pulling", None).await;
                        this.ports.engine.pull_image(image, &context).await.map_err(ContainersError::from)
                    }
                };
                this.record_update(&target, done.is_ok(), Some(&who), &context).await;
                if let Err(error) = this.check_updates(&context).await {
                    tracing::warn!(%error, "containers: cannot check for newer images after updating");
                }
                done.map_err(|error| job_error(&error))
            })
            .await?;
        Ok(job)
    }
}
