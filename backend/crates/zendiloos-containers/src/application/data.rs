//! Moving Docker's data to another pool, as a whole: images, containers,
//! networks and volumes. Apps' own data stays on its pool.

use zendiloos_jobs::Job;
use zendiloos_kernel::{Actor, RequestContext};

use super::ports::DataPlace;
use super::{Containers, ContainersError, job_error};

/// Where Docker's data is and could go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLocation {
    /// `None`: not on a pool zendiloOS made, so it cannot be moved here.
    pub current: Option<DataPlace>,
    /// The other pools.
    pub targets: Vec<DataPlace>,
    /// What Docker's data takes up.
    pub data_bytes: u64,
}

impl Containers {
    pub async fn data_location(&self, context: &RequestContext) -> Result<DataLocation, ContainersError> {
        let (current, places) = self.ports.data.places(context).await?;
        let data_bytes = self.ports.engine.disk_usage(context).await.map(|d| d.total_bytes()).unwrap_or(0);
        let targets = places
            .into_iter()
            .filter(|place| Some(&place.pool) != current.as_ref().map(|c| &c.pool))
            .collect();
        Ok(DataLocation { current, targets, data_bytes })
    }

    /// Moves Docker's data to `pool`, in a job. Every container stops
    /// meanwhile; those that restart by themselves start again after.
    /// Refused while a volume is backed up or restored (or waits for its
    /// turn), and none is until the move ended.
    pub async fn move_data(
        &self,
        pool: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<Job, ContainersError> {
        let location = self.data_location(context).await?;
        let from = location.current.ok_or_else(|| {
            ContainersError::Refused("Docker's data is not on a pool zendiloOS made".into())
        })?;
        let to =
            location.targets.into_iter().find(|place| place.pool == pool).ok_or(ContainersError::NotFound)?;
        if to.available_bytes.is_some_and(|free| free < location.data_bytes) {
            return Err(ContainersError::Refused(format!(
                "{pool} has too little free space for Docker's data"
            )));
        }
        self.begin_move()?;
        let this = self.clone();
        let (who, context) = (actor.clone(), context.clone());
        let target = format!("{} → {}", from.pool, to.pool);
        let job = self
            .ports
            .jobs
            .start("containers.data_move", Some(target.clone()), Some(actor), move |progress| async move {
                progress.step("moving", None).await;
                let done = this.ports.data.move_to(&from, &to, &context).await.map_err(Into::into);
                this.end_move();
                let done = this.audited("containers.data_move", &target, &who, &context, done).await;
                done.map_err(|error| job_error(&error))
            })
            .await;
        if job.is_err() {
            self.end_move();
        }
        Ok(job?)
    }
}
