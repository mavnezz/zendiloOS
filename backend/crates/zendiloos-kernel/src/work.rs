/// What runs in the background now (jobs: copies, permission changes,
/// changes of pools, installs), each named for a person to read, e.g.
/// `sharing.apply_access photos`.
///
/// Updates of zendiloOS and reboots ask it first: an agent seeing a long
/// operation through would keep a package upgrade waiting, and a reboot
/// would cut it off.
pub trait RunningWork: Send + Sync {
    fn running(&self) -> Vec<String>;
}

/// Nothing runs; for tests and tools without background work.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoWork;

impl RunningWork for NoWork {
    fn running(&self) -> Vec<String> {
        Vec::new()
    }
}
