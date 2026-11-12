//! Signing in to Google Drive and OneDrive for backups (docs/architecture.md,
//! 4.14): OAuth's authorization code flow without rclone on the
//! administrator's computer. The consent page sends the browser on to
//! `http://127.0.0.1:53682/` (Google) or `http://localhost:53682/`
//! (Microsoft), where nothing listens: the administrator pastes the address
//! back, and zendiloosd exchanges its code here for a token, written as
//! rclone keeps one. rclone's own public clients are asked for unless the
//! administrator names one of their own; rclone refreshes the token as the
//! backups run.

use std::sync::Arc;

use async_trait::async_trait;
use hyper::Method;
use hyper::header::{ACCEPT, AUTHORIZATION};
use serde::Deserialize;
use zendiloos_backup::application::{CloudSignIn, HostError};
use zendiloos_backup::domain::Provider;
use zendiloos_kernel::{Clock, Secret};

use crate::https;

/// rclone's public OAuth clients, as rclone ships them: their consent
/// pages name rclone, and a token they gave rclone refreshes.
const GOOGLE_CLIENT_ID: &str = "202264815644.apps.googleusercontent.com";
const GOOGLE_CLIENT_SECRET: &str = "X4Z3ca8xfWDb1Voo-F9a7ZxJ";
const ONEDRIVE_CLIENT_ID: &str = "b15665d9-eda6-4092-8539-0eec376afd59";
const ONEDRIVE_CLIENT_SECRET: &str = "qtyfaBBYA403=unZUP40~_#";
/// Where the consent pages send the browser, as rclone's clients allow.
const GOOGLE_REDIRECT: &str = "http://127.0.0.1:53682/";
const ONEDRIVE_REDIRECT: &str = "http://localhost:53682/";
/// rclone's `drive.file`: what rclone made, nothing else of the drive.
const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const ONEDRIVE_SCOPES: &str =
    "Files.Read Files.ReadWrite Files.Read.All Files.ReadWrite.All offline_access Sites.Read.All";
const ANSWER_BYTES: usize = 64 * 1024;

/// Where the providers' pages are; others only in tests.
#[derive(Debug, Clone)]
pub struct OAuthEndpoints {
    pub google_consent: String,
    pub google_token: String,
    pub microsoft_consent: String,
    pub microsoft_token: String,
    /// Microsoft Graph, which names a OneDrive account's drive.
    pub graph: String,
}

impl Default for OAuthEndpoints {
    fn default() -> Self {
        Self {
            google_consent: "https://accounts.google.com/o/oauth2/auth".into(),
            google_token: "https://oauth2.googleapis.com/token".into(),
            microsoft_consent: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            microsoft_token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            graph: "https://graph.microsoft.com/v1.0".into(),
        }
    }
}

/// [`CloudSignIn`] over https.
pub struct OAuthSignIn {
    tls: Arc<rustls::ClientConfig>,
    endpoints: OAuthEndpoints,
    clock: Arc<dyn Clock>,
}

impl OAuthSignIn {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with(https::client_config(), OAuthEndpoints::default(), clock)
    }

    pub(crate) fn with(
        tls: Arc<rustls::ClientConfig>,
        endpoints: OAuthEndpoints,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { tls, endpoints, clock }
    }

    /// The client asked for: the administrator's own, or rclone's.
    fn client(provider: Provider, own: Option<&str>) -> (&str, &'static str) {
        let redirect = match provider {
            Provider::GoogleDrive => GOOGLE_REDIRECT,
            Provider::OneDrive => ONEDRIVE_REDIRECT,
        };
        let id = own.unwrap_or(match provider {
            Provider::GoogleDrive => GOOGLE_CLIENT_ID,
            Provider::OneDrive => ONEDRIVE_CLIENT_ID,
        });
        (id, redirect)
    }
}

/// What a token endpoint answers.
#[derive(Debug, Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    token_type: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DriveAnswer {
    id: String,
    #[serde(rename = "driveType")]
    drive_type: String,
}

/// `at` as RFC 3339 in UTC, as Go's OAuth library reads a token's expiry.
fn rfc3339(at: zendiloos_kernel::Timestamp) -> String {
    let t = at.utc();
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

#[async_trait]
impl CloudSignIn for OAuthSignIn {
    fn new_state(&self) -> String {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn consent_url(&self, provider: Provider, client_id: Option<&str>, state: &str) -> String {
        let (id, redirect) = Self::client(provider, client_id);
        let mut query = vec![("client_id", id), ("redirect_uri", redirect), ("response_type", "code")];
        let base = match provider {
            Provider::GoogleDrive => {
                query.extend([("scope", GOOGLE_SCOPE), ("access_type", "offline"), ("prompt", "consent")]);
                &self.endpoints.google_consent
            }
            Provider::OneDrive => {
                query.push(("scope", ONEDRIVE_SCOPES));
                &self.endpoints.microsoft_consent
            }
        };
        query.push(("state", state));
        format!("{base}?{}", https::form_encoded(&query))
    }

    async fn exchange(
        &self,
        provider: Provider,
        client_id: Option<&str>,
        client_secret: Option<&Secret>,
        code: &str,
    ) -> Result<Secret, HostError> {
        let (id, redirect) = Self::client(provider, client_id);
        let secret = match (client_id, client_secret) {
            (Some(_), secret) => secret.map(Secret::expose),
            (None, _) => Some(match provider {
                Provider::GoogleDrive => GOOGLE_CLIENT_SECRET,
                Provider::OneDrive => ONEDRIVE_CLIENT_SECRET,
            }),
        };
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect),
            ("client_id", id),
        ];
        if let Some(secret) = secret {
            form.push(("client_secret", secret));
        }
        let address = match provider {
            Provider::GoogleDrive => &self.endpoints.google_token,
            Provider::OneDrive => &self.endpoints.microsoft_token,
        };
        let accept = [(ACCEPT, "application/json".to_owned())];
        let answer = https::post_form(&self.tls, address, &accept, &form, ANSWER_BYTES)
            .await
            .map_err(|e| HostError::Failed(format!("the sign-in cannot be completed: {e}")))?;
        let token: TokenAnswer = serde_json::from_slice(&answer.body).map_err(|_| {
            HostError::Failed(format!("the sign-in answered {} with what cannot be read", answer.status))
        })?;
        let refused = |token: &TokenAnswer| {
            let said = token.error_description.as_deref().or(token.error.as_deref()).unwrap_or("no token");
            HostError::Refused(format!("the sign-in was refused: {said}"))
        };
        if !answer.status.is_success() {
            return Err(refused(&token));
        }
        let (Some(access), Some(refresh)) = (&token.access_token, &token.refresh_token) else {
            return Err(refused(&token));
        };
        let expires_in = token.expires_in.unwrap_or(3600).clamp(60, 24 * 3600);
        let expiry =
            self.clock.now().saturating_add(std::time::Duration::from_secs(expires_in.unsigned_abs()));
        let kept = serde_json::json!({
            "access_token": access,
            "token_type": token.token_type.as_deref().unwrap_or("Bearer"),
            "refresh_token": refresh,
            "expiry": rfc3339(expiry),
        });
        Ok(Secret::new(kept.to_string()))
    }

    async fn drive(&self, provider: Provider, token: &Secret) -> Result<Option<(String, String)>, HostError> {
        if provider != Provider::OneDrive {
            return Ok(None);
        }
        let access = serde_json::from_str::<serde_json::Value>(token.expose())
            .ok()
            .and_then(|token| token.get("access_token").and_then(|a| a.as_str()).map(str::to_owned))
            .ok_or_else(|| HostError::Failed("the token holds no access token".into()))?;
        let address = format!("{}/me/drive", self.endpoints.graph);
        let headers = [(AUTHORIZATION, format!("Bearer {access}")), (ACCEPT, "application/json".to_owned())];
        let answer = https::send(&self.tls, Method::GET, &address, &headers, ANSWER_BYTES)
            .await
            .map_err(|e| HostError::Failed(format!("OneDrive's drive cannot be asked for: {e}")))?;
        if !answer.status.is_success() {
            return Err(HostError::Refused(format!(
                "OneDrive answered {} for the account's drive",
                answer.status
            )));
        }
        let drive: DriveAnswer = serde_json::from_slice(&answer.body)
            .map_err(|_| HostError::Failed("OneDrive named no drive".into()))?;
        Ok(Some((drive.id, drive.drive_type)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::test_https::{Asked, client, response, serve};

    struct At(Timestamp);

    impl Clock for At {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    /// A fake of both providers' token endpoints and Graph, recording what
    /// was asked.
    async fn providers() -> (OAuthSignIn, Arc<Mutex<Vec<Asked>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let port = serve(move |request: &Asked| {
            seen.lock().unwrap().push(request.clone());
            match (request.method.as_str(), request.path.as_str()) {
                ("POST", "/token") if request.body.contains("code=good") => response(
                    "200 OK",
                    &[("Content-Type", "application/json")],
                    br#"{"access_token":"ya29.a","token_type":"Bearer","refresh_token":"1//r","expires_in":3599}"#,
                ),
                ("POST", "/token") if request.body.contains("code=no-refresh") => {
                    response("200 OK", &[], br#"{"access_token":"ya29.a","token_type":"Bearer"}"#)
                }
                ("POST", "/token") => response(
                    "400 Bad Request",
                    &[],
                    br#"{"error":"invalid_grant","error_description":"Bad Request: the code was used"}"#,
                ),
                ("GET", "/graph/me/drive") if request.headers.contains("authorization: bearer ya29.a") => {
                    response("200 OK", &[], br#"{"id":"b!drive-1","driveType":"business","owner":{}}"#)
                }
                _ => response("401 Unauthorized", &[], b"{}"),
            }
        })
        .await;
        let at = |path: &str| format!("https://127.0.0.1:{port}{path}");
        let endpoints = OAuthEndpoints {
            google_consent: "https://accounts.example/auth".into(),
            google_token: at("/token"),
            microsoft_consent: "https://login.example/authorize".into(),
            microsoft_token: at("/token"),
            graph: at("/graph"),
        };
        let clock: Arc<dyn Clock> = Arc::new(At(Timestamp::from_unix_seconds(1_794_380_400)));
        (OAuthSignIn::with(client(), endpoints, clock), asked)
    }

    #[tokio::test]
    async fn the_consent_pages_ask_for_rclones_client_unless_another_is_given() {
        let (sign_in, _) = providers().await;
        let google = sign_in.consent_url(Provider::GoogleDrive, None, "s1");
        assert_eq!(
            google,
            "https://accounts.example/auth?client_id=202264815644.apps.googleusercontent.com\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2F&response_type=code\
             &scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file&access_type=offline&prompt=consent&state=s1"
        );
        let onedrive = sign_in.consent_url(Provider::OneDrive, Some("my-own-client"), "s2");
        assert!(
            onedrive.starts_with("https://login.example/authorize?client_id=my-own-client&"),
            "{onedrive}"
        );
        assert!(onedrive.contains("redirect_uri=http%3A%2F%2Flocalhost%3A53682%2F"), "{onedrive}");
        assert!(onedrive.contains("offline_access") && onedrive.ends_with("&state=s2"), "{onedrive}");
        assert_eq!(sign_in.new_state().len(), 32);
        assert_ne!(sign_in.new_state(), sign_in.new_state());
    }

    #[tokio::test]
    async fn a_code_becomes_a_token_as_rclone_keeps_one() {
        let (sign_in, asked) = providers().await;
        let token = sign_in.exchange(Provider::OneDrive, None, None, "good").await.unwrap();
        let token: serde_json::Value = serde_json::from_str(token.expose()).unwrap();
        assert_eq!(token["access_token"], "ya29.a");
        assert_eq!(token["refresh_token"], "1//r");
        assert_eq!(token["token_type"], "Bearer");
        // 2026-11-11 07:00 UTC and an hour, as Go's OAuth library reads it.
        assert_eq!(token["expiry"], "2026-11-11T07:59:59Z");
        let body = asked.lock().unwrap()[0].body.clone();
        assert!(body.starts_with("grant_type=authorization_code&code=good&"), "{body}");
        assert!(body.contains("redirect_uri=http%3A%2F%2Flocalhost%3A53682%2F"), "{body}");
        assert!(body.contains("client_id=b15665d9-eda6-4092-8539-0eec376afd59"), "{body}");
        assert!(body.contains("client_secret=qtyfaBBYA403%3DunZUP40~_%23"), "{body}");
        assert!(asked.lock().unwrap()[0].headers.contains("content-type: application/x-www-form-urlencoded"));

        let drive =
            sign_in.drive(Provider::OneDrive, &Secret::new(r#"{"access_token":"ya29.a"}"#)).await.unwrap();
        assert_eq!(drive, Some(("b!drive-1".to_owned(), "business".to_owned())));
        assert_eq!(sign_in.drive(Provider::GoogleDrive, &Secret::new("{}")).await.unwrap(), None);
        let other = sign_in.drive(Provider::OneDrive, &Secret::new(r#"{"access_token":"other"}"#)).await;
        assert!(matches!(other, Err(HostError::Refused(_))), "{other:?}");
    }

    #[tokio::test]
    async fn an_own_client_is_used_and_refusals_are_told_in_the_providers_words() {
        let (sign_in, asked) = providers().await;
        let own = Secret::new("own secret");
        sign_in.exchange(Provider::GoogleDrive, Some("own.apps"), Some(&own), "good").await.unwrap();
        let body = asked.lock().unwrap()[0].body.clone();
        assert!(body.contains("client_id=own.apps&client_secret=own%20secret"), "{body}");
        assert!(body.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2F"), "{body}");

        let used = sign_in.exchange(Provider::GoogleDrive, None, None, "used").await;
        assert!(
            matches!(&used, Err(HostError::Refused(said)) if said.contains("the code was used")),
            "{used:?}"
        );
        let short = sign_in.exchange(Provider::GoogleDrive, None, None, "no-refresh").await;
        assert!(matches!(short, Err(HostError::Refused(_))), "{short:?}");
    }
}
