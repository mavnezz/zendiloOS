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
mod feature_tests;
#[cfg(test)]
mod tests;

use axum::Router;
use axum::extract::{FromRequestParts, State};
use axum::http::Request;
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;

pub use auth::CSRF_HEADER;
pub use error::{ApiError, Problem};
pub use state::{ApiState, UseCases};

/// Prefix of the current API version.
pub const API_V1: &str = "/api/v1";

#[derive(OpenApi)]
#[openapi(
    info(title = "Zendiloos API", description = "Management API of the Zendiloos NAS operating system."),
    tags(
        (name = "system", description = "Health and information about the host"),
        (name = "identity", description = "Initial setup, login and sessions"),
        (name = "accounts", description = "Accounts and groups, managed by administrators"),
        (name = "storage", description = "Disks, SMART, RAID arrays and volumes"),
        (name = "logs", description = "The system journal and the audit log"),
        (name = "jobs", description = "Long-running operations and their progress"),
        (name = "shares", description = "Shares over SMB and NFS and their permissions"),
        (name = "files", description = "The file manager: share contents as the signed-in user"),
        (name = "certificates", description = "The ACME client plugin: accounts, challenges and certificates"),
    ),
    components(schemas(
        routes::live::LiveRequest,
        routes::live::LiveMessage,
        routes::logs::LogSource,
        routes::terminal::TerminalRequest,
        routes::terminal::TerminalMessage,
    )),
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
        .merge(routes::terminal::router())
        .fallback(|| async { ApiError::NotFound })
        .method_not_allowed_fallback(|| async { ApiError::MethodNotAllowed })
        .layer(from_fn(run_to_the_end))
        .layer(from_fn_with_state(state.clone(), refuse_blocked))
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

/// Runs a request that changes something to its end even if its client
/// hangs up meanwhile (a tab closed; nginx gives up after a minute): hyper
/// drops the handler then, and a change cut off halfway would leave the
/// host and the records apart, a Linux account without its record, say.
/// Requests that only read (and WebSocket upgrades, which are GETs) go with
/// their client.
async fn run_to_the_end(request: axum::extract::Request, next: Next) -> Response {
    if request.method().is_safe() {
        return next.run(request).await;
    }
    match tokio::spawn(next.run(request)).await {
        Ok(response) => response,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => ApiError::internal(error).into_response(),
    }
}

/// Refuses every request from an address the web interface blocks; the
/// health check stays open for monitoring.
async fn refuse_blocked(
    State(state): State<ApiState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if request.uri().path() == format!("{API_V1}/health") {
        return next.run(request).await;
    }
    let (mut parts, body) = request.into_parts();
    let client = match extract::Client::from_request_parts(&mut parts, &state).await {
        Ok(extract::Client(client)) => client,
        Err(never) => match never {},
    };
    match state.identity.check_address.is_blocked(client.client_ip).await {
        Ok(false) => next.run(axum::extract::Request::from_parts(parts, body)).await,
        Ok(true) => ApiError::address_blocked().into_response(),
        Err(error) => ApiError::internal(error).into_response(),
    }
}

#[cfg(test)]
mod to_the_end {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn a_change_runs_to_its_end_when_the_client_hangs_up_a_read_does_not() {
        let (changed, read) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let slow = |flag: Arc<AtomicBool>| {
            move || async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                flag.store(true, Ordering::SeqCst);
            }
        };
        let app = Router::new()
            .route("/x", post(slow(changed.clone())).get(slow(read.clone())))
            .layer(from_fn(run_to_the_end));
        for method in ["POST", "GET"] {
            let request =
                axum::http::Request::builder().method(method).uri("/x").body(axum::body::Body::empty());
            // The client gives up after 10 ms.
            let _ =
                tokio::time::timeout(Duration::from_millis(10), app.clone().oneshot(request.unwrap())).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(changed.load(Ordering::SeqCst), "the change went on");
        assert!(!read.load(Ordering::SeqCst), "the read went with its client");
    }
}
