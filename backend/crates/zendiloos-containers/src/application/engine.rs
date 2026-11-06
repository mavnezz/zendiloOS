//! The engine's containers, images and networks.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};

use super::{Containers, ContainersError, job_error};
use crate::domain::{
    BUILT_IN_NETWORKS, Container, ContainerAction, ContainerUsage, Image, Network, assemble,
};

/// The engine at a glance.
#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub projects_running: usize,
    pub projects: usize,
    pub containers_running: usize,
    pub containers: usize,
    pub images: usize,
    /// What Docker's data takes up.
    pub data_bytes: u64,
    /// Of the machine's processors, 0 to 100; `None` before the first sample.
    pub cpu_share: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContainerView {
    pub container: Container,
    /// From the newest sample, while it runs.
    pub usage: Option<ContainerUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageView {
    pub image: Image,
    /// The containers made from it, by name.
    pub used_by: Vec<String>,
}

impl Containers {
    pub async fn overview(&self, context: &RequestContext) -> Result<Overview, ContainersError> {
        let engine = &self.ports.engine;
        let containers = engine.containers(context).await?;
        let listings = engine.compose_projects(context).await?;
        let images = engine.images(context).await?;
        let disk = engine.disk_usage(context).await?;
        let projects = assemble(self.ports.projects.list().await?, &listings, &containers);
        let latest = self.history().latest().cloned();
        Ok(Overview {
            projects_running: projects.iter().filter(|p| p.running() > 0).count(),
            projects: projects.len(),
            containers_running: containers.iter().filter(|c| c.state.is_up()).count(),
            containers: containers.len(),
            images: images.len(),
            data_bytes: disk.total_bytes(),
            cpu_share: latest.as_ref().map(crate::domain::UsageSample::cpu_share),
            memory_bytes: latest.as_ref().map(crate::domain::UsageSample::memory_bytes),
            memory_total_bytes: latest.as_ref().map(crate::domain::UsageSample::memory_total_bytes),
        })
    }

    /// Every container, by name, with its newest use.
    pub async fn containers(&self, context: &RequestContext) -> Result<Vec<ContainerView>, ContainersError> {
        let mut containers = self.ports.engine.containers(context).await?;
        containers.sort_by(|a, b| a.name.cmp(&b.name));
        let latest = self.history().latest().cloned();
        Ok(containers
            .into_iter()
            .map(|container| {
                let usage = latest
                    .as_ref()
                    .filter(|_| container.state.is_up())
                    .and_then(|sample| sample.containers.iter().find(|u| u.id == container.id).cloned());
                ContainerView { container, usage }
            })
            .collect())
    }

    async fn find_container(&self, id: &str, context: &RequestContext) -> Result<Container, ContainersError> {
        let containers = self.ports.engine.containers(context).await?;
        containers.into_iter().find(|c| c.id == id).ok_or(ContainersError::NotFound)
    }

    pub async fn control_container(
        &self,
        id: &str,
        action: ContainerAction,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let container = self.find_container(id, context).await?;
        let done =
            self.ports.engine.control_container(&container.id, action, context).await.map_err(Into::into);
        let audited = match action {
            ContainerAction::Start => "containers.container_start",
            ContainerAction::Stop => "containers.container_stop",
            ContainerAction::Restart => "containers.container_restart",
            ContainerAction::Remove => "containers.container_remove",
        };
        self.audited(audited, &container.name, actor, context, done).await
    }

    pub async fn container_logs(
        &self,
        id: &str,
        lines: u32,
        context: &RequestContext,
    ) -> Result<String, ContainersError> {
        let container = self.find_container(id, context).await?;
        Ok(self.ports.engine.container_logs(&container.id, lines, context).await?)
    }

    /// Every image, the newest first, with the containers made from it.
    pub async fn images(&self, context: &RequestContext) -> Result<Vec<ImageView>, ContainersError> {
        let mut images = self.ports.engine.images(context).await?;
        images.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| a.id.cmp(&b.id)));
        let containers = self.ports.engine.containers(context).await?;
        Ok(images
            .into_iter()
            .map(|image| {
                let used_by =
                    containers.iter().filter(|c| c.image_id == image.id).map(|c| c.name.clone()).collect();
                ImageView { image, used_by }
            })
            .collect())
    }

    pub async fn remove_image(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let images = self.ports.engine.images(context).await?;
        let image = images.iter().find(|i| i.id == id).ok_or(ContainersError::NotFound)?;
        let done = self.ports.engine.remove_image(&image.id, context).await.map_err(Into::into);
        self.audited("containers.image_remove", &image.label(), actor, context, done).await
    }

    /// Fetches an image, in a job: it can take long.
    pub async fn pull_image(
        &self,
        reference: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let reference = reference.trim().to_owned();
        let valid = !reference.is_empty()
            && reference.len() <= 255
            && reference.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
            && reference.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/:@-".contains(&b));
        if !valid {
            return Err(ContainersError::Invalid(format!(
                "{reference:?} is no image name such as postgres:17"
            )));
        }
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let target = reference.clone();
        let job = self
            .ports
            .jobs
            .start(
                "containers.image_pull",
                Some(reference.clone()),
                Some(actor),
                move |progress| async move {
                    progress.step("pulling", None).await;
                    let done = this.ports.engine.pull_image(&reference, &context).await.map_err(Into::into);
                    let done = this.audited("containers.image_pull", &target, &who, &context, done).await;
                    done.map_err(|error| job_error(&error))
                },
            )
            .await?;
        Ok(job)
    }

    pub async fn prune_images(
        &self,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<String, ContainersError> {
        let done = self.ports.engine.prune_images(context).await.map_err(Into::into);
        self.audited("containers.image_prune", "unused images", actor, context, done).await
    }

    /// Every network, Docker's own first, then by name.
    pub async fn networks(&self, context: &RequestContext) -> Result<Vec<Network>, ContainersError> {
        let mut networks = self.ports.engine.networks(context).await?;
        networks.sort_by(|a, b| (!a.is_built_in(), &a.name).cmp(&(!b.is_built_in(), &b.name)));
        Ok(networks)
    }

    pub async fn create_network(
        &self,
        name: &str,
        subnet: Option<&str>,
        gateway: Option<&str>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        let mut bytes = name.bytes();
        let valid = (1..=64).contains(&name.len())
            && bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
            && bytes.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
        if !valid {
            return Err(ContainersError::Invalid(format!("{name:?} is no network name")));
        }
        if gateway.is_some() && subnet.is_none() {
            return Err(ContainersError::Invalid("a gateway needs a subnet".into()));
        }
        let done = self.ports.engine.create_network(name, subnet, gateway, context).await.map_err(Into::into);
        self.audited("containers.network_create", name, actor, context, done).await
    }

    pub async fn remove_network(
        &self,
        name: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), ContainersError> {
        if BUILT_IN_NETWORKS.contains(&name) {
            return Err(ContainersError::Refused(format!("{name} is Docker's own network")));
        }
        let networks = self.ports.engine.networks(context).await?;
        let network = networks.iter().find(|n| n.name == name).ok_or(ContainersError::NotFound)?;
        let done = self.ports.engine.remove_network(&network.name, context).await.map_err(Into::into);
        self.audited("containers.network_remove", name, actor, context, done).await
    }
}
