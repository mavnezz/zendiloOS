mod accounts;
mod apps;
mod certificates;
mod channels;
mod docker;
mod docker_volumes;
mod enclosure;
mod files;
mod firewall;
mod identity;
pub(crate) mod jobs;
pub(crate) mod live;
pub(crate) mod logs;
mod network;
mod removable;
mod security;
mod shares;
mod storage;
mod system;
pub(crate) mod terminal;
mod two_factor;
mod updates;
pub(crate) mod vm_console;
mod vm_images;
mod vms;

use utoipa_axum::router::OpenApiRouter;

use crate::state::ApiState;

/// All routes of API version 1, relative to [`crate::API_V1`].
pub(crate) fn v1() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .merge(system::routes())
        .merge(identity::routes())
        .merge(two_factor::routes())
        .merge(storage::routes())
        .merge(enclosure::routes())
        .merge(removable::routes())
        .merge(accounts::routes())
        .merge(logs::routes())
        .merge(jobs::routes())
        .merge(shares::routes())
        .merge(files::routes())
        .merge(network::routes())
        .merge(channels::routes())
        .merge(apps::routes())
        .merge(security::routes())
        .merge(docker::routes())
        .merge(docker_volumes::routes())
        .merge(updates::routes())
        .merge(certificates::routes())
        .merge(firewall::routes())
        .merge(vms::routes())
}
