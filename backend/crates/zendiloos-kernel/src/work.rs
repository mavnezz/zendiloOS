use std::sync::Arc;

use async_trait::async_trait;

/// What runs in the background now (jobs: copies, permission changes,
/// changes of pools, installs; an update of the system), each named for a
/// person to read, e.g. `sharing.apply_access photos`.
///
/// Updates of zendiloOS and reboots ask it first: an agent seeing a long
/// operation through would keep a package upgrade waiting, and a reboot
/// would cut it off, an update's apt and dpkg among them.
#[async_trait]
pub trait RunningWork: Send + Sync {
    async fn running(&self) -> Vec<String>;
}

/// Nothing runs; for tests and tools without background work.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoWork;

#[async_trait]
impl RunningWork for NoWork {
    async fn running(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Several kinds of background work, as one.
#[derive(Clone, Default)]
pub struct AllWork(pub Vec<Arc<dyn RunningWork>>);

#[async_trait]
impl RunningWork for AllWork {
    async fn running(&self) -> Vec<String> {
        let mut all = Vec::new();
        for work in &self.0 {
            all.extend(work.running().await);
        }
        all
    }
}
