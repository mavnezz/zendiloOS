//! Compose projects: listed whoever made them; made, changed and deployed
//! here by administrators, with their Compose file in a share.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, AuditEntry, Outcome, RequestContext};

use super::ports::{EngineError, ProjectAction, ProjectPlace};
use super::{Containers, ContainersError, job_error};
use crate::domain::{COMPOSE_FILE, Origin, OwnProject, Project, ProjectName, Risk, assemble, risks};

/// A project to make: its folder in a share and its Compose file.
#[derive(Debug, Clone)]
pub struct NewProject {
    pub name: String,
    pub share_id: i64,
    /// In the share: `docker/teslamate`; made if missing.
    pub folder: String,
    pub compose: String,
    /// Deploys it right away.
    pub start: bool,
    /// The administrator accepted what the file lets containers do.
    pub accept_risks: bool,
}

/// A project's Compose files and whether they are edited here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeSource {
    /// Path on the machine and content, in Compose's order.
    pub files: Vec<(String, String)>,
    pub editable: bool,
}

/// A folder in a share as given, tidied: `docker/teslamate`.
fn folder(text: &str) -> Result<String, ContainersError> {
    let trimmed = text.trim().trim_matches('/');
    let parts: Vec<&str> = trimmed.split('/').collect();
    let valid = !trimmed.is_empty()
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part.len() <= 255
                && !part.chars().any(char::is_control)
        });
    if valid {
        Ok(parts.join("/"))
    } else {
        Err(ContainersError::Invalid(format!("{text:?} is no folder in a share such as docker/teslamate")))
    }
}

/// How a project is brought up.
#[derive(Debug, Clone)]
pub(super) enum Deployment {
    /// An administrator's, from what Compose made of the file they
    /// confirmed.
    Own(String),
    /// An administrator's made before that was kept: not deployed until its
    /// file is saved once.
    Unconfirmed,
    /// Made another way: from the files Compose reports.
    Foreign,
}

impl Deployment {
    pub(super) fn of(project: &OwnProject) -> Self {
        project.config.clone().map_or(Self::Unconfirmed, Self::Own)
    }

    /// Pulls (if `pull`) and brings the project up.
    pub(super) async fn run(
        &self,
        containers: &Containers,
        name: &str,
        pull: bool,
        context: &RequestContext,
    ) -> Result<(), EngineError> {
        let engine = &containers.ports.engine;
        match self {
            Self::Own(config) => engine.deploy_project(name, config, pull, context).await,
            Self::Unconfirmed => Err(EngineError::Refused(ContainersError::Unconfirmed.to_string())),
            Self::Foreign => {
                if pull {
                    engine.control_project(name, ProjectAction::Pull, context).await?;
                }
                engine.control_project(name, ProjectAction::Up, context).await
            }
        }
    }
}

impl Containers {
    /// Every project, by name.
    pub async fn projects(&self, context: &RequestContext) -> Result<Vec<Project>, ContainersError> {
        let engine = &self.ports.engine;
        let containers = engine.containers(context).await?;
        let listings = engine.compose_projects(context).await?;
        Ok(assemble(self.ports.projects.list().await?, &listings, &containers))
    }

    pub async fn project(&self, name: &str, context: &RequestContext) -> Result<Project, ContainersError> {
        let projects = self.projects(context).await?;
        projects.into_iter().find(|p| p.name == name).ok_or(ContainersError::NotFound)
    }

    /// The shares `actor` may put a project's folder in.
    pub async fn places(&self, actor: &Actor) -> Result<Vec<ProjectPlace>, ContainersError> {
        Ok(self.ports.storage.places(actor).await?)
    }

    /// What `compose` lets containers do, if Compose can read it, and what
    /// Compose made of it, which is what will run; refused unless `accept`
    /// while it reaches beyond the containers.
    async fn checked(
        &self,
        name: &ProjectName,
        directory: &str,
        compose: &str,
        accept: bool,
        context: &RequestContext,
    ) -> Result<(Vec<Risk>, String), ContainersError> {
        let checked = self.ports.engine.check_compose(name, directory, compose, context).await?;
        let found = risks(&checked.model);
        if !found.is_empty() && !accept {
            return Err(ContainersError::Risky(found));
        }
        Ok((found, checked.config))
    }

    /// What `compose` would let the containers do, if Compose can read it:
    /// for a new project in `place` (share and folder), or for an existing
    /// one of an administrator's.
    pub async fn check_compose(
        &self,
        name: &str,
        place: Option<(i64, &str)>,
        compose: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Vec<Risk>, ContainersError> {
        let name = ProjectName::parse(name.trim())?;
        let directory = match place {
            Some((share_id, place)) => self.ports.storage.directory(actor, share_id, &folder(place)?).await?,
            None => self.own(name.as_str(), context).await?.directory,
        };
        Ok(self.checked(&name, &directory, compose, true, context).await?.0)
    }

    async fn record(
        &self,
        action: &'static str,
        name: &str,
        risky: bool,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let mut entry =
            AuditEntry::new(self.now(), action, Outcome::Success, context).by(actor).target(name.to_owned());
        if risky {
            entry = entry.reason("risks_accepted");
        }
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "containers: cannot write the audit log");
        }
    }

    /// Makes a project: checks its Compose file, writes it into the folder
    /// and, with `start`, deploys it in a job.
    pub async fn create_project(
        &self,
        new: NewProject,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Option<Job>, ContainersError> {
        let name = ProjectName::parse(new.name.trim())?;
        let folder = folder(&new.folder)?;
        if self.projects(context).await?.iter().any(|p| p.name == name.as_str()) {
            return Err(ContainersError::NameTaken);
        }
        let storage = &self.ports.storage;
        let directory = storage.directory(actor, new.share_id, &folder).await?;
        let (found, config) =
            self.checked(&name, &directory, &new.compose, new.accept_risks, context).await?;
        storage.write(actor, new.share_id, &folder, COMPOSE_FILE, &new.compose, context).await?;
        let project = OwnProject {
            name: name.clone(),
            share_id: new.share_id,
            folder,
            file: format!("{directory}/{COMPOSE_FILE}"),
            directory,
            compose: Some(new.compose.clone()),
            config: Some(config.clone()),
            created_at: self.now(),
            created_by: actor.name.clone(),
        };
        if !self.ports.projects.create(&project).await? {
            return Err(ContainersError::NameTaken);
        }
        self.record("containers.project_create", name.as_str(), !found.is_empty(), actor, context).await;
        if !new.start {
            return Ok(None);
        }
        Ok(Some(self.deploy(name.to_string(), Deployment::Own(config), false, actor, context).await?))
    }

    /// Pulls (if `pull`) and brings up a project's containers, in a job.
    async fn deploy(
        &self,
        name: String,
        how: Deployment,
        pull: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let job = self
            .ports
            .jobs
            .start("containers.project_deploy", Some(name.clone()), Some(actor), move |progress| async move {
                progress.step(if pull { "pulling" } else { "starting" }, Some(0.1)).await;
                let done = how.run(&this, &name, pull, &context).await;
                let done = this
                    .audited("containers.project_deploy", &name, &who, &context, done.map_err(Into::into))
                    .await;
                done.map_err(|error| job_error(&error))
            })
            .await?;
        Ok(job)
    }

    /// An administrator's project, refused for the others.
    async fn own(&self, name: &str, context: &RequestContext) -> Result<OwnProject, ContainersError> {
        match self.project(name, context).await?.origin {
            Origin::Own(project) => Ok(project),
            Origin::App { .. } => Err(ContainersError::AppProject),
            Origin::Foreign { .. } => Err(ContainersError::NotOwn),
        }
    }

    /// Writes a project's changed Compose file and deploys it again.
    pub async fn change_compose(
        &self,
        name: &str,
        compose: &str,
        accept_risks: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let project = self.own(name, context).await?;
        let (found, config) =
            self.checked(&project.name, &project.directory, compose, accept_risks, context).await?;
        let storage = &self.ports.storage;
        storage.write(actor, project.share_id, &project.folder, COMPOSE_FILE, compose, context).await?;
        if !self.ports.projects.confirm(&project.name, compose, &config).await? {
            return Err(ContainersError::NotFound);
        }
        self.record("containers.project_change", name, !found.is_empty(), actor, context).await;
        self.deploy(name.to_owned(), Deployment::Own(config), false, actor, context).await
    }

    /// The project's Compose files: an administrator's as they confirmed it
    /// (one made before that was kept: from its share, read with their
    /// permissions); another's as Compose reports them.
    pub async fn project_compose(
        &self,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<ComposeSource, ContainersError> {
        match self.project(name, context).await?.origin {
            Origin::Own(project) => {
                let text = match project.compose {
                    Some(text) => text,
                    None => {
                        let path = format!("{}/{COMPOSE_FILE}", project.folder);
                        self.ports.storage.read(actor, project.share_id, &path, context).await?
                    }
                };
                Ok(ComposeSource { files: vec![(project.file, text)], editable: true })
            }
            Origin::App { .. } => Err(ContainersError::AppProject),
            Origin::Foreign { .. } => {
                let files = self.ports.engine.project_files(name, context).await?;
                Ok(ComposeSource { files: files.into_iter().collect(), editable: false })
            }
        }
    }

    /// Starts, stops or restarts a project's containers.
    pub async fn control_project(
        &self,
        name: &str,
        action: ProjectAction,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        if !matches!(action, ProjectAction::Start | ProjectAction::Stop | ProjectAction::Restart) {
            return Err(ContainersError::Invalid("only start, stop and restart".into()));
        }
        let project = self.project(name, context).await?;
        let done =
            self.ports.engine.control_project(&project.name, action, context).await.map_err(Into::into);
        let audited = match action {
            ProjectAction::Start => "containers.project_start",
            ProjectAction::Stop => "containers.project_stop",
            _ => "containers.project_restart",
        };
        self.audited(audited, name, actor, context, done).await
    }

    /// Fetches the images anew and recreates what changed, in a job.
    pub async fn redeploy(
        &self,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let how = match self.project(name, context).await?.origin {
            Origin::Own(project) => Deployment::of(&project),
            Origin::App { .. } => return Err(ContainersError::AppProject),
            Origin::Foreign { .. } => Deployment::Foreign,
        };
        if matches!(how, Deployment::Unconfirmed) {
            return Err(ContainersError::Unconfirmed);
        }
        self.deploy(name.to_owned(), how, true, actor, context).await
    }

    /// Removes a project's containers and networks, in a job; images, data
    /// and its folder stay. A project made here is forgotten too.
    pub async fn delete_project(
        &self,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let project = self.project(name, context).await?;
        if matches!(project.origin, Origin::App { .. }) {
            return Err(ContainersError::AppProject);
        }
        let own = match &project.origin {
            Origin::Own(own) => Some(own.name.clone()),
            _ => None,
        };
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let name = name.to_owned();
        let job = self
            .ports
            .jobs
            .start("containers.project_delete", Some(name.clone()), Some(actor), move |progress| async move {
                progress.step("removing", None).await;
                let mut done: Result<(), ContainersError> = Ok(());
                if !project.containers.is_empty() {
                    done = this
                        .ports
                        .engine
                        .control_project(&name, ProjectAction::Down, &context)
                        .await
                        .map_err(Into::into);
                }
                if let (Ok(()), Some(own)) = (&done, &own) {
                    done = this.ports.projects.delete(own).await.map(|_| ()).map_err(Into::into);
                }
                let done = this.audited("containers.project_delete", &name, &who, &context, done).await;
                done.map_err(|error| job_error(&error))
            })
            .await?;
        Ok(job)
    }

    pub async fn project_logs(
        &self,
        name: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, ContainersError> {
        let project = self.project(name, context).await?;
        Ok(self.ports.engine.project_logs(&project.name, lines, context).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::folder;

    #[test]
    fn folders_are_tidied_and_stay_in_their_share() {
        assert_eq!(folder(" /docker/teslamate/ ").unwrap(), "docker/teslamate");
        for bad in ["", "/", "docker/../etc", "docker//x", "./docker", "a\nb"] {
            assert!(folder(bad).is_err(), "{bad:?}");
        }
    }
}
