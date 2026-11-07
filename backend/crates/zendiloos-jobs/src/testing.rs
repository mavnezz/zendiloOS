//! An in-memory [`JobRepository`] and a fixed clock, for tests.

use std::sync::Mutex;

use async_trait::async_trait;
use zendiloos_kernel::{Clock, PortError, Timestamp};

use crate::{Job, JobId, JobRepository, JobState, NewJob};

#[derive(Default)]
pub struct MemoryJobs {
    jobs: Mutex<Vec<Job>>,
}

impl MemoryJobs {
    pub fn all(&self) -> Vec<Job> {
        self.jobs.lock().unwrap().clone()
    }
}

#[async_trait]
impl JobRepository for MemoryJobs {
    async fn create(&self, new: NewJob) -> Result<Job, PortError> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = Job {
            id: JobId::new(i64::try_from(jobs.len()).unwrap() + 1),
            kind: new.kind,
            target: new.target,
            state: JobState::Running,
            step: None,
            progress: None,
            error: None,
            account_id: new.account_id,
            actor: new.actor,
            created_at: new.created_at,
            updated_at: new.created_at,
            finished_at: None,
        };
        jobs.push(job.clone());
        Ok(job)
    }

    async fn update(&self, job: &Job) -> Result<(), PortError> {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(stored) = jobs.iter_mut().find(|j| j.id == job.id) {
            *stored = job.clone();
        }
        Ok(())
    }

    async fn find(&self, id: JobId) -> Result<Option<Job>, PortError> {
        Ok(self.jobs.lock().unwrap().iter().find(|j| j.id == id).cloned())
    }

    async fn recent(&self, limit: u32) -> Result<Vec<Job>, PortError> {
        let jobs = self.jobs.lock().unwrap();
        Ok(jobs.iter().rev().take(limit as usize).cloned().collect())
    }

    async fn recent_of(&self, account: i64, limit: u32) -> Result<Vec<Job>, PortError> {
        let jobs = self.jobs.lock().unwrap();
        Ok(jobs
            .iter()
            .rev()
            .filter(|job| job.account_id == Some(account))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError> {
        let mut count = 0;
        for job in self.jobs.lock().unwrap().iter_mut().filter(|j| j.state == JobState::Running) {
            job.state = JobState::Interrupted;
            job.finished_at = Some(at);
            job.updated_at = at;
            count += 1;
        }
        Ok(count)
    }
}

/// Always the same instant.
pub struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_seconds(1_793_880_000)
    }
}
