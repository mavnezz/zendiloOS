use zendiloos_identity::application::IdentityUseCases;
use zendiloos_storage::application::StorageUseCases;
use zendiloos_system::application::SystemUseCases;

/// The use cases the handlers call, and how the API is exposed. Cheap to
/// clone.
#[derive(Clone)]
pub struct ApiState {
    pub(crate) system: SystemUseCases,
    pub(crate) identity: IdentityUseCases,
    pub(crate) storage: StorageUseCases,
    /// Session cookies get `Secure` and the `__Host-` prefix. Only turn
    /// this off where the interface is reached over plain HTTP.
    pub(crate) secure_cookies: bool,
}

/// The use cases of all contexts.
pub struct UseCases {
    pub system: SystemUseCases,
    pub identity: IdentityUseCases,
    pub storage: StorageUseCases,
}

impl ApiState {
    pub fn new(use_cases: UseCases, secure_cookies: bool) -> Self {
        let UseCases { system, identity, storage } = use_cases;
        Self { system, identity, storage, secure_cookies }
    }
}
