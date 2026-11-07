//! Extractors shared by the handlers.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Request};
use axum::http::header::USER_AGENT;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;
use zendiloos_kernel::RequestContext;

use crate::error::ApiError;

/// `axum::Json`, but failures are answered as problem details.
pub struct ApiJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => {
                Err(ApiError::InvalidBody { status: rejection.status(), detail: rejection.body_text() })
            }
        }
    }
}

/// `axum::extract::Query`, but failures are answered as problem details.
pub struct ApiQuery<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Query(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::BadRequest(rejection.body_text())),
        }
    }
}

/// `axum::extract::Path`, but failures (an id that is no number) are
/// answered as problem details. Routes import it as `Path`.
pub struct ApiPath<T>(pub T);

impl<T: DeserializeOwned + Send, S: Send + Sync> FromRequestParts<S> for ApiPath<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Self(value)),
            Err(rejection) => Err(ApiError::BadRequest(rejection.body_text())),
        }
    }
}

/// Where the request came from, for audit and throttling.
pub struct Client(pub RequestContext);

const MAX_USER_AGENT_CHARS: usize = 256;

impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = |name| parts.headers.get(name).and_then(|value| value.to_str().ok());
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(addr)| addr.ip());
        // Only the reverse proxy on this machine may say who the client is:
        // over TCP from loopback, or over the Unix socket that only nginx can
        // open (no peer address).
        let forwarded = header("x-real-ip").and_then(|value| value.parse::<IpAddr>().ok());
        let client_ip = match peer {
            Some(peer) if peer.is_loopback() => forwarded.or(Some(peer)),
            Some(peer) => Some(peer),
            None => forwarded,
        };
        Ok(Client(RequestContext {
            client_ip,
            user_agent: header(USER_AGENT.as_str()).map(|ua| ua.chars().take(MAX_USER_AGENT_CHARS).collect()),
            request_id: header("x-request-id").map(str::to_owned),
        }))
    }
}
