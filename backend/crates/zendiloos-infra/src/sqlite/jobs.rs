use async_trait::async_trait;
use zendiloos_jobs::{Job, JobError, JobId, JobRepository, JobState, NewJob};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`JobRepository`] on the `jobs` table.
#[derive(Debug, Clone)]
pub struct SqliteJobs {
    db: Database,
}

impl SqliteJobs {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("jobs", error)
}

struct JobRow {
    id: i64,
    kind: String,
    target: Option<String>,
    state: String,
    step: Option<String>,
    progress: Option<f64>,
    error_code: Option<String>,
    error_detail: Option<String>,
    account_id: Option<i64>,
    actor: Option<String>,
    created_at: i64,
    updated_at: i64,
    finished_at: Option<i64>,
}

impl TryFrom<JobRow> for Job {
    type Error = PortError;

    fn try_from(row: JobRow) -> Result<Self, PortError> {
        let state = JobState::parse(&row.state)
            .ok_or_else(|| PortError::new("stored job", format!("unknown state {:?}", row.state)))?;
        Ok(Job {
            id: JobId::new(row.id),
            kind: row.kind,
            target: row.target,
            state,
            step: row.step,
            progress: row.progress.map(|p| p as f32),
            error: row.error_code.map(|code| JobError { code, detail: row.error_detail }),
            account_id: row.account_id,
            actor: row.actor,
            created_at: Timestamp::from_unix_seconds(row.created_at),
            updated_at: Timestamp::from_unix_seconds(row.updated_at),
            finished_at: row.finished_at.map(Timestamp::from_unix_seconds),
        })
    }
}

#[async_trait]
impl JobRepository for SqliteJobs {
    async fn create(&self, new: NewJob) -> Result<Job, PortError> {
        let at = new.created_at.unix_seconds();
        let id = sqlx::query_scalar!(
            r#"INSERT INTO jobs (kind, target, state, account_id, actor, created_at, updated_at)
               VALUES (?1, ?2, 'running', ?3, ?4, ?5, ?5)
               RETURNING id AS "id!""#,
            new.kind,
            new.target,
            new.account_id,
            new.actor,
            at
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(Job {
            id: JobId::new(id),
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
        })
    }

    async fn update(&self, job: &Job) -> Result<(), PortError> {
        let (id, state) = (job.id.get(), job.state.as_str());
        let progress = job.progress.map(f64::from);
        let (code, detail) = match &job.error {
            Some(error) => (Some(error.code.as_str()), error.detail.as_deref()),
            None => (None, None),
        };
        let (updated, finished) = (job.updated_at.unix_seconds(), job.finished_at.map(|t| t.unix_seconds()));
        sqlx::query!(
            "UPDATE jobs SET state = ?, step = ?, progress = ?, error_code = ?, error_detail = ?,
                             updated_at = ?, finished_at = ?
             WHERE id = ?",
            state,
            job.step,
            progress,
            code,
            detail,
            updated,
            finished,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn find(&self, id: JobId) -> Result<Option<Job>, PortError> {
        let id = id.get();
        sqlx::query_as!(
            JobRow,
            r#"SELECT id AS "id!", kind, target, state, step, progress, error_code, error_detail,
                      account_id, actor, created_at, updated_at, finished_at
               FROM jobs WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        .map(Job::try_from)
        .transpose()
    }

    async fn recent(&self, limit: u32) -> Result<Vec<Job>, PortError> {
        let limit = i64::from(limit);
        sqlx::query_as!(
            JobRow,
            r#"SELECT id AS "id!", kind, target, state, step, progress, error_code, error_detail,
                      account_id, actor, created_at, updated_at, finished_at
               FROM jobs ORDER BY id DESC LIMIT ?"#,
            limit
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(Job::try_from)
        .collect()
    }

    async fn recent_of(&self, account: i64, limit: u32) -> Result<Vec<Job>, PortError> {
        let limit = i64::from(limit);
        sqlx::query_as!(
            JobRow,
            r#"SELECT id AS "id!", kind, target, state, step, progress, error_code, error_detail,
                      account_id, actor, created_at, updated_at, finished_at
               FROM jobs WHERE account_id = ? ORDER BY id DESC LIMIT ?"#,
            account,
            limit
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(Job::try_from)
        .collect()
    }

    async fn interrupt_running(&self, at: Timestamp) -> Result<u64, PortError> {
        let at = at.unix_seconds();
        let result = sqlx::query!(
            "UPDATE jobs SET state = 'interrupted', updated_at = ?1, finished_at = ?1 WHERE state = 'running'",
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn new_job(kind: &str) -> NewJob {
        NewJob {
            kind: kind.to_owned(),
            target: Some("tank".to_owned()),
            account_id: Some(1),
            actor: Some("alice".to_owned()),
            created_at: Timestamp::from_unix_seconds(100),
        }
    }

    #[tokio::test]
    async fn stores_jobs_and_interrupts_running_ones() {
        let (_dir, db) = temporary_database().await;
        let jobs = SqliteJobs::new(db.clone());

        let mut done = jobs.create(new_job("storage.create_pool")).await.unwrap();
        let running = jobs.create(new_job("storage.replace_disk")).await.unwrap();
        done.state = JobState::Failed;
        done.step = Some("partitioning".to_owned());
        done.progress = Some(0.25);
        done.error = Some(JobError::new("agent_failed", Some("sgdisk failed".to_owned())));
        done.finished_at = Some(Timestamp::from_unix_seconds(160));
        jobs.update(&done).await.unwrap();

        assert_eq!(jobs.find(done.id).await.unwrap(), Some(done.clone()));
        assert_eq!(jobs.interrupt_running(Timestamp::from_unix_seconds(200)).await.unwrap(), 1);
        let recent = jobs.recent(10).await.unwrap();
        assert_eq!(recent.iter().map(|j| j.id).collect::<Vec<_>>(), [running.id, done.id]);
        assert_eq!(recent[0].state, JobState::Interrupted);
        assert_eq!(recent[1].state, JobState::Failed, "finished jobs stay as they are");
        db.close().await;
    }
}
