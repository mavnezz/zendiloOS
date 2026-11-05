//! Session cookies, CSRF protection and the extractors that authenticate a
//! request.
//!
//! A handler is protected by taking [`Auth`] (or [`AuthAllowPending`]) as
//! its first argument. A test checks that every route not explicitly public
//! answers 401 without a session.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use zendiloos_identity::application::{AuthContext, SignedIn};
use zendiloos_identity::domain::Permission;
use zendiloos_kernel::{Secret, constant_time_eq};

use crate::error::ApiError;
use crate::state::ApiState;

/// Header carrying the session's CSRF token on state-changing requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// With `Secure`, the `__Host-` prefix makes browsers refuse the cookie
/// unless it is host-only, `Secure` and for path `/`.
fn cookie_name(secure: bool) -> &'static str {
    if secure { "__Host-zendiloos_session" } else { "zendiloos_session" }
}

/// The cookie that carries a new session's token. It has no `Max-Age`, so
/// it ends with the browser session; idle timeout and lifetime are enforced
/// on the server.
pub(crate) fn session_cookie(signed_in: &SignedIn, secure: bool) -> Cookie<'static> {
    Cookie::build((cookie_name(secure), signed_in.token.expose().to_owned()))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .path("/")
        .build()
}

/// Tells the browser to drop the session cookie.
pub(crate) fn removal_cookie(secure: bool) -> Cookie<'static> {
    let mut cookie =
        Cookie::build((cookie_name(secure), "")).http_only(true).secure(secure).path("/").build();
    cookie.make_removal();
    cookie
}

/// Rejects requests a browser sent on behalf of another site. Needed even
/// before login: otherwise a foreign page could sign the victim into an
/// account the attacker controls.
pub(crate) fn check_same_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(site) = header("sec-fetch-site")
        && !matches!(site, "same-origin" | "none")
    {
        return Err(ApiError::CsrfRejected);
    }
    if let Some(origin) = header(header::ORIGIN.as_str()) {
        let origin_host = origin.split_once("://").map(|(_, host)| host);
        if origin_host.is_none() || origin_host != header(header::HOST.as_str()) {
            return Err(ApiError::CsrfRejected);
        }
    }
    Ok(())
}

async fn authenticate(parts: &Parts, state: &ApiState, allow_pending: bool) -> Result<AuthContext, ApiError> {
    let jar = CookieJar::from_headers(&parts.headers);
    let token = jar.get(cookie_name(state.secure_cookies)).ok_or(ApiError::Unauthenticated)?;
    let auth = state.identity.authenticate.execute(&Secret::new(token.value())).await?;

    if !parts.method.is_safe() {
        check_same_origin(&parts.headers)?;
        let sent = parts.headers.get(CSRF_HEADER).map(|value| value.as_bytes()).unwrap_or_default();
        if !constant_time_eq(sent, auth.csrf_token.expose().as_bytes()) {
            return Err(ApiError::CsrfRejected);
        }
    }
    if auth.account.password_change_required && !allow_pending {
        return Err(ApiError::PasswordChangeRequired);
    }
    Ok(auth)
}

/// The signed-in caller. Rejects the request unless it carries a valid
/// session, a valid CSRF token if it changes state, and the account has no
/// pending password change.
pub struct Auth(pub AuthContext);

impl Auth {
    pub fn require(&self, permission: Permission) -> Result<(), ApiError> {
        if self.0.can(permission) { Ok(()) } else { Err(ApiError::Forbidden) }
    }
}

impl FromRequestParts<ApiState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, Self::Rejection> {
        authenticate(parts, state, false).await.map(Self)
    }
}

/// Like [`Auth`], but also lets accounts with a pending password change
/// through. Only for what they need to change it: the session itself, the
/// password change, and logout.
pub struct AuthAllowPending(pub AuthContext);

impl FromRequestParts<ApiState> for AuthAllowPending {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &ApiState) -> Result<Self, Self::Rejection> {
        authenticate(parts, state, true).await.map(Self)
    }
}

/// Requests that sign someone in must come from our own pages.
pub struct SameOrigin;

impl<S: Send + Sync> FromRequestParts<S> for SameOrigin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        check_same_origin(&parts.headers).map(|()| Self)
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs.iter().map(|(k, v)| (header::HeaderName::from_static(k), HeaderValue::from_static(v))).collect()
    }

    #[test]
    fn same_origin_requests_pass() {
        assert!(check_same_origin(&headers(&[])).is_ok(), "non-browser clients send neither header");
        assert!(
            check_same_origin(&headers(&[("host", "nas.local"), ("origin", "https://nas.local")])).is_ok()
        );
        assert!(check_same_origin(&headers(&[("sec-fetch-site", "same-origin")])).is_ok());
    }

    #[test]
    fn cross_site_requests_are_rejected() {
        for headers in [
            headers(&[("host", "nas.local"), ("origin", "https://evil.example")]),
            headers(&[("host", "nas.local"), ("origin", "null")]),
            headers(&[("host", "nas.local:8443"), ("origin", "https://nas.local")]),
            headers(&[("sec-fetch-site", "cross-site")]),
            headers(&[("sec-fetch-site", "same-site")]),
        ] {
            assert!(matches!(check_same_origin(&headers), Err(ApiError::CsrfRejected)), "{headers:?}");
        }
    }
}
