use std::error::Error;
use std::fmt::Write as _;
use std::time::Duration;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;
use zendiloos_identity::application::IdentityError;

/// Error body as defined by RFC 9457 (`application/problem+json`).
#[derive(Debug, Serialize, ToSchema)]
pub struct Problem {
    /// Always `about:blank`; `code` identifies the problem.
    #[serde(rename = "type")]
    pub kind: String,
    /// Reason phrase of the status code.
    pub title: String,
    pub status: u16,
    /// Human-readable explanation, if there is more to say than the title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Stable, machine-readable identifier the frontend can rely on, e.g. `not_found`.
    pub code: String,
}

/// Every error a handler can return. Internal details are logged, never sent
/// to the client.
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    MethodNotAllowed,
    /// The request is malformed in a way the other variants do not cover.
    BadRequest(String),
    /// The request body could not be read as the expected JSON.
    InvalidBody {
        status: StatusCode,
        detail: String,
    },
    /// The input was understood but breaks a rule; `detail` says which.
    InvalidInput(String),
    Unauthenticated,
    Forbidden,
    /// The account must change its password before doing anything else.
    PasswordChangeRequired,
    /// Missing or wrong CSRF token, or a cross-site request.
    CsrfRejected,
    InvalidCredentials,
    WrongPassword,
    AccountDisabled,
    TooManyAttempts {
        retry_after: Duration,
    },
    SetupCompleted,
    InvalidSetupCode,
    /// The name is used by another account or group, or by the system.
    NameTaken,
    /// The change would leave no enabled administrator.
    LastAdmin,
    /// Administrators change their own account in their settings.
    OwnAccount,
    Internal(Box<dyn Error + Send + Sync>),
}

impl ApiError {
    pub fn internal(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self::Internal(error.into())
    }
}

impl From<zendiloos_kernel::PortError> for ApiError {
    fn from(error: zendiloos_kernel::PortError) -> Self {
        Self::internal(error)
    }
}

impl From<IdentityError> for ApiError {
    fn from(error: IdentityError) -> Self {
        match error {
            IdentityError::InvalidAccountName(e) => ApiError::InvalidInput(e.to_string()),
            IdentityError::PasswordRejected(e) => ApiError::InvalidInput(e.to_string()),
            IdentityError::InvalidCredentials => ApiError::InvalidCredentials,
            IdentityError::WrongPassword => ApiError::WrongPassword,
            IdentityError::AccountDisabled => ApiError::AccountDisabled,
            IdentityError::TooManyAttempts { retry_after } => ApiError::TooManyAttempts { retry_after },
            IdentityError::SetupCompleted => ApiError::SetupCompleted,
            IdentityError::InvalidSetupCode => ApiError::InvalidSetupCode,
            IdentityError::Unauthenticated => ApiError::Unauthenticated,
            IdentityError::InvalidText(e) => ApiError::InvalidInput(e.to_string()),
            IdentityError::Forbidden => ApiError::Forbidden,
            IdentityError::AccountNotFound | IdentityError::GroupNotFound => ApiError::NotFound,
            IdentityError::NameTaken => ApiError::NameTaken,
            IdentityError::LastAdmin => ApiError::LastAdmin,
            IdentityError::OwnAccount => ApiError::OwnAccount,
            IdentityError::Port(e) => ApiError::internal(e),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut retry_after = None;
        let (status, code, detail) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", None),
            ApiError::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", None),
            ApiError::BadRequest(detail) => (StatusCode::BAD_REQUEST, "bad_request", Some(detail)),
            ApiError::InvalidBody { status, detail } => (status, "invalid_body", Some(detail)),
            ApiError::InvalidInput(detail) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", Some(detail))
            }
            ApiError::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated", None),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", None),
            ApiError::PasswordChangeRequired => (StatusCode::FORBIDDEN, "password_change_required", None),
            ApiError::CsrfRejected => (StatusCode::FORBIDDEN, "csrf_rejected", None),
            ApiError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid_credentials", None),
            ApiError::WrongPassword => (StatusCode::UNPROCESSABLE_ENTITY, "wrong_password", None),
            ApiError::AccountDisabled => (StatusCode::FORBIDDEN, "account_disabled", None),
            ApiError::TooManyAttempts { retry_after: after } => {
                retry_after = Some(after.as_secs().max(1));
                (StatusCode::TOO_MANY_REQUESTS, "too_many_attempts", None)
            }
            ApiError::SetupCompleted => (StatusCode::CONFLICT, "setup_completed", None),
            ApiError::InvalidSetupCode => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_setup_code", None),
            ApiError::NameTaken => (StatusCode::CONFLICT, "name_taken", None),
            ApiError::LastAdmin => (StatusCode::CONFLICT, "last_admin", None),
            ApiError::OwnAccount => (StatusCode::CONFLICT, "own_account", None),
            ApiError::Internal(error) => {
                tracing::error!(error = %error_chain(error.as_ref()), "request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
            }
        };
        let problem = Problem {
            kind: "about:blank".to_owned(),
            title: status.canonical_reason().unwrap_or_default().to_owned(),
            status: status.as_u16(),
            detail,
            code: code.to_owned(),
        };
        let mut response =
            (status, [(header::CONTENT_TYPE, "application/problem+json")], Json(problem)).into_response();
        if let Some(seconds) = retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

/// `outer: inner: innermost`, so the log shows the root cause.
fn error_chain(error: &(dyn Error + 'static)) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let _ = write!(out, ": {cause}");
        source = cause.source();
    }
    out
}
