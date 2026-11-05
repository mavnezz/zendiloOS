use zendiloos_identity::application::IdentityUseCases;
use zendiloos_jobs::Jobs;
use zendiloos_notify::Notifier;
use zendiloos_sharing::application::{FilesUseCases, SharingUseCases};
use zendiloos_storage::application::StorageUseCases;
use zendiloos_system::application::SystemUseCases;

/// The use cases the handlers call, and how the API is exposed. Cheap to
/// clone.
#[derive(Clone)]
pub struct ApiState {
    pub(crate) system: SystemUseCases,
    pub(crate) identity: IdentityUseCases,
    pub(crate) storage: StorageUseCases,
    pub(crate) sharing: SharingUseCases,
    pub(crate) files: FilesUseCases,
    pub(crate) jobs: Jobs,
    pub(crate) notifier: Notifier,
    /// Session cookies get `Secure` and the `__Host-` prefix. Only turn
    /// this off where the interface is reached over plain HTTP.
    pub(crate) secure_cookies: bool,
}

/// The use cases of all contexts.
pub struct UseCases {
    pub system: SystemUseCases,
    pub identity: IdentityUseCases,
    pub storage: StorageUseCases,
    pub sharing: SharingUseCases,
    pub files: FilesUseCases,
    pub jobs: Jobs,
    pub notifier: Notifier,
}

impl ApiState {
    pub fn new(use_cases: UseCases, secure_cookies: bool) -> Self {
        let UseCases { system, identity, storage, sharing, files, jobs, notifier } = use_cases;
        Self { system, identity, storage, sharing, files, jobs, notifier, secure_cookies }
    }
}
