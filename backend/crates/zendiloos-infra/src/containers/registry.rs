//! [`Registries`] over the registry API (OCI distribution): a `HEAD` of the
//! tag's manifest answers with its digest, without pulling anything and
//! without counting against Docker Hub's limit for pulls. Registries that
//! want a token give one to anyone for public images.

use std::sync::Arc;

use async_trait::async_trait;
use hyper::Method;
use hyper::header::{ACCEPT, AUTHORIZATION, HeaderName, WWW_AUTHENTICATE};
use zendiloos_containers::application::ports::Registries;
use zendiloos_containers::domain::RegistryImage;

use crate::https::{self, Answer};

/// Every kind of manifest a tag can name: a list for several platforms, or
/// one platform's.
const MANIFESTS: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.oci.image.manifest.v1+json";
const DIGEST: &str = "docker-content-digest";
const MAX_TOKEN: usize = 64 * 1024;

pub struct HttpsRegistries {
    tls: Arc<rustls::ClientConfig>,
}

impl Default for HttpsRegistries {
    fn default() -> Self {
        Self { tls: https::client_config() }
    }
}

impl HttpsRegistries {
    async fn head(&self, address: &str, token: Option<&str>) -> Result<Answer, String> {
        let mut headers: Vec<(HeaderName, String)> = vec![(ACCEPT, MANIFESTS.to_owned())];
        if let Some(token) = token {
            headers.push((AUTHORIZATION, format!("Bearer {token}")));
        }
        https::send(&self.tls, Method::HEAD, address, &headers, 0).await
    }

    /// The anonymous token a `Bearer` challenge points to.
    async fn token(&self, challenge: &str) -> Result<String, String> {
        let realm = parameter(challenge, "realm").ok_or("the registry names no place for a token")?;
        let mut address = realm.clone();
        let mut separator = if realm.contains('?') { '&' } else { '?' };
        for key in ["service", "scope"] {
            if let Some(value) = parameter(challenge, key) {
                address.push_str(&format!("{separator}{key}={}", encode(&value)));
                separator = '&';
            }
        }
        let answer = https::send(&self.tls, Method::GET, &address, &[], MAX_TOKEN).await?;
        if !answer.status.is_success() {
            return Err(format!("the registry gives no token: {}", answer.status));
        }
        let body: serde_json::Value = serde_json::from_slice(&answer.body).map_err(|e| e.to_string())?;
        body.get("token")
            .or_else(|| body.get("access_token"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the registry's answer holds no token".to_owned())
    }
}

/// `realm="https://auth.docker.io/token",service="registry.docker.io"`:
/// the value of `key`.
fn parameter(challenge: &str, key: &str) -> Option<String> {
    let rest = challenge.trim().strip_prefix("Bearer ").unwrap_or(challenge);
    rest.split(',').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == key).then(|| value.trim_matches('"').to_owned())
    })
}

/// Percent-encodes what a query value may not hold as it is.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn digest_of(answer: &Answer) -> Option<String> {
    answer.headers.get(DIGEST).and_then(|d| d.to_str().ok()).map(str::to_owned)
}

#[async_trait]
impl Registries for HttpsRegistries {
    async fn digest(&self, image: &RegistryImage) -> Result<String, String> {
        let address = format!("https://{}/v2/{}/manifests/{}", image.registry, image.repository, image.tag);
        let mut answer = self.head(&address, None).await?;
        if answer.status.as_u16() == 401 {
            let challenge = answer
                .headers
                .get(WWW_AUTHENTICATE)
                .and_then(|c| c.to_str().ok())
                .ok_or("the registry wants a login")?
                .to_owned();
            let token = self.token(&challenge).await?;
            answer = self.head(&address, Some(&token)).await?;
        }
        match answer.status.as_u16() {
            200 => digest_of(&answer).ok_or_else(|| "the registry names no digest".to_owned()),
            401 | 403 => Err("the registry wants a login".to_owned()),
            404 => Err("the registry does not know this tag".to_owned()),
            429 => Err("the registry asks to wait".to_owned()),
            status => Err(format!("the registry answered {status}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_name_where_a_token_comes_from() {
        let challenge = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/postgres:pull""#;
        assert_eq!(parameter(challenge, "realm").as_deref(), Some("https://auth.docker.io/token"));
        assert_eq!(parameter(challenge, "scope").as_deref(), Some("repository:library/postgres:pull"));
        assert_eq!(parameter(challenge, "error"), None);
        assert_eq!(encode("repository:library/postgres:pull"), "repository%3Alibrary%2Fpostgres%3Apull");
    }
}
