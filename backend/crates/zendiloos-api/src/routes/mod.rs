mod accounts;
mod apps;
mod channels;
mod docker;
mod files;
mod identity;
pub(crate) mod jobs;
pub(crate) mod live;
pub(crate) mod logs;
mod network;
mod security;
mod shares;
mod storage;
mod system;
mod updates;

use utoipa_axum::router::OpenApiRouter;

use crate::state::ApiState;

/// All routes of API version 1, relative to [`crate::API_V1`].
pub(crate) fn v1() -> OpenApiRouter<ApiState> {
    OpenApiRouter::new()
        .merge(system::routes())
        .merge(identity::routes())
        .merge(storage::routes())
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
        .merge(updates::routes())
}
