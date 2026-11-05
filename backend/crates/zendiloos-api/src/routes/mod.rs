mod accounts;
mod files;
mod identity;
pub(crate) mod jobs;
pub(crate) mod live;
pub(crate) mod logs;
mod shares;
mod storage;
mod system;

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
}
