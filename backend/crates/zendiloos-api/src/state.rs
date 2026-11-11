use zendiloos_apps::application::{Catalogs, Docker, Installs, Templates};
use zendiloos_certificates::application::Certificates;
use zendiloos_containers::application::Containers;
use zendiloos_firewall::application::Firewall;
use zendiloos_identity::application::IdentityUseCases;
use zendiloos_jobs::Jobs;
use zendiloos_network::application::NetworkUseCases;
use zendiloos_notify::Notifier;
use zendiloos_notify::channels::Channels;
use zendiloos_sharing::application::{FilesUseCases, SharingUseCases};
use zendiloos_storage::application::StorageUseCases;
use zendiloos_system::application::SystemUseCases;
use zendiloos_updates::application::Updates;
use zendiloos_vms::application::Vms;

/// The use cases the handlers call, and how the API is exposed. Cheap to
/// clone.
#[derive(Clone)]
pub struct ApiState {
    pub(crate) system: SystemUseCases,
    pub(crate) identity: IdentityUseCases,
    pub(crate) storage: StorageUseCases,
    pub(crate) sharing: SharingUseCases,
    pub(crate) files: FilesUseCases,
    pub(crate) network: NetworkUseCases,
    pub(crate) jobs: Jobs,
    pub(crate) notifier: Notifier,
    pub(crate) channels: Channels,
    pub(crate) apps: Catalogs,
    pub(crate) docker: Docker,
    pub(crate) installs: Installs,
    pub(crate) templates: Templates,
    pub(crate) containers: Containers,
    pub(crate) updates: Updates,
    pub(crate) certificates: Certificates,
    pub(crate) firewall: Firewall,
    pub(crate) vms: Vms,
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
    pub network: NetworkUseCases,
    pub jobs: Jobs,
    pub notifier: Notifier,
    pub channels: Channels,
    pub apps: Catalogs,
    pub docker: Docker,
    pub installs: Installs,
    pub templates: Templates,
    pub containers: Containers,
    pub updates: Updates,
    pub certificates: Certificates,
    pub firewall: Firewall,
    pub vms: Vms,
}

impl ApiState {
    pub fn new(use_cases: UseCases, secure_cookies: bool) -> Self {
        let UseCases {
            system,
            identity,
            storage,
            sharing,
            files,
            network,
            jobs,
            notifier,
            channels,
            apps,
            docker,
            installs,
            templates,
            containers,
            updates,
            certificates,
            firewall,
            vms,
        } = use_cases;
        Self {
            system,
            identity,
            storage,
            sharing,
            files,
            network,
            jobs,
            notifier,
            channels,
            apps,
            docker,
            installs,
            templates,
            containers,
            updates,
            certificates,
            firewall,
            vms,
            secure_cookies,
        }
    }
}
