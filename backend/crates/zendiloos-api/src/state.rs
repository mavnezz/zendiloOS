use zendiloos_identity::application::IdentityUseCases;
use zendiloos_system::application::SystemUseCases;

/// The use cases the handlers call, and how the API is exposed. Cheap to
/// clone.
#[derive(Clone)]
pub struct ApiState {
    pub(crate) system: SystemUseCases,
    pub(crate) identity: IdentityUseCases,
    /// Session cookies get `Secure` and the `__Host-` prefix. Only turn
    /// this off where the interface is reached over plain HTTP.
    pub(crate) secure_cookies: bool,
}

impl ApiState {
    pub fn new(system: SystemUseCases, identity: IdentityUseCases, secure_cookies: bool) -> Self {
        Self { system, identity, secure_cookies }
    }
}
