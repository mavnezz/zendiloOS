//! HTTP API of Zendiloos.
//!
//! Every route is registered through [`utoipa_axum`], so the OpenAPI document
//! ([`openapi`]) always matches the router ([`router`]). The frontend's
//! TypeScript types are generated from that document.

mod auth;
mod error;
mod extract;
mod routes;
mod state;

#[cfg(test)]
mod tests;

use axum::Router;
use axum::http::Request;
use axum::routing::get;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;

pub use auth::CSRF_HEADER;
pub use error::{ApiError, Problem};
pub use state::ApiState;

/// Prefix of the current API version.
pub const API_V1: &str = "/api/v1";

#[derive(OpenApi)]
#[openapi(
    info(title = "Zendiloos API", description = "Management API of the Zendiloos NAS operating system."),
    tags(
        (name = "system", description = "Health and information about the host"),
        (name = "identity", description = "Initial setup, login and sessions"),
    ),
    components(schemas(routes::live::LiveRequest, routes::live::LiveMessage)),
    modifiers(&SessionSecurity)
)]
struct ApiDoc;

/// Documents the session cookie. Operations that need it say
/// `security(("session" = []))`; state-changing ones additionally need the
/// session's CSRF token in the `X-CSRF-Token` header.
struct SessionSecurity;

impl Modify for SessionSecurity {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "__Host-zendiloos_session",
                "Set by login or setup. Named `zendiloos_session` where secure cookies are off.",
            ))),
        );
    }
}

fn api() -> OpenApiRouter<ApiState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi()).nest(API_V1, routes::v1())
}

/// The OpenAPI document describing [`router`].
pub fn openapi() -> utoipa::openapi::OpenApi {
    api().split_for_parts().1
}

/// The complete HTTP application, ready to be served.
pub fn router(state: ApiState) -> Router {
    let (router, openapi) = api().split_for_parts();
    let openapi_json = openapi.to_json().expect("the OpenAPI document serializes");

    router
        .route(
            &format!("{API_V1}/openapi.json"),
            get(move || async move { ([(axum::http::header::CONTENT_TYPE, "application/json")], openapi_json) }),
        )
        .merge(routes::live::router())
        .fallback(|| async { ApiError::NotFound })
        .method_not_allowed_fallback(|| async { ApiError::MethodNotAllowed })
        .with_state(state)
        // Layers wrap everything added before them: the request ID is set
        // first, then the request is traced, and the ID is copied to the
        // response last.
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
            let request_id =
                request.headers().get("x-request-id").and_then(|value| value.to_str().ok()).unwrap_or("");
            // The path only: query strings may carry tokens.
            tracing::info_span!("request", method = %request.method(), path = %request.uri().path(), request_id)
        }))
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
