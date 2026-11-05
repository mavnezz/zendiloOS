use std::sync::Arc;

use zendiloos_kernel::PortError;

use crate::application::ports::{AuditReader, JournalReader};
use crate::domain::{AuditQuery, AuditRecord, JournalPage, JournalQuery};

/// Query: a page of the system journal. The caller has checked the
/// permission; the limit is capped here.
#[derive(Clone)]
pub struct ReadJournal {
    journal: Arc<dyn JournalReader>,
}

impl ReadJournal {
    pub fn new(journal: Arc<dyn JournalReader>) -> Self {
        Self { journal }
    }

    pub async fn execute(&self, mut query: JournalQuery) -> Result<JournalPage, PortError> {
        query.limit = query.limit.clamp(1, JournalQuery::MAX_LIMIT);
        self.journal.read(&query).await
    }
}

/// Query: a page of the audit log, newest first.
#[derive(Clone)]
pub struct ReadAuditLog {
    audit: Arc<dyn AuditReader>,
}

impl ReadAuditLog {
    pub fn new(audit: Arc<dyn AuditReader>) -> Self {
        Self { audit }
    }

    pub async fn execute(&self, mut query: AuditQuery) -> Result<Vec<AuditRecord>, PortError> {
        query.limit = query.limit.clamp(1, AuditQuery::MAX_LIMIT);
        self.audit.read(&query).await
    }
}
