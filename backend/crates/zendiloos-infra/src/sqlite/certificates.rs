use std::collections::BTreeMap;

use async_trait::async_trait;
use zendiloos_certificates::application::CertificateRepository;
use zendiloos_certificates::domain::{
    AcmeAccount, AcmeServer, Attempt, Certificate, Challenge, ChallengeMethod, KeyType, NewAccount,
    NewCertificate, WebServer,
};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`CertificateRepository`] on `acme_accounts`, `acme_challenges`,
/// `certificates` and `web_server`.
#[derive(Debug, Clone)]
pub struct SqliteCertificates {
    db: Database,
}

impl SqliteCertificates {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("certificates", error)
}

fn stored(what: &'static str) -> PortError {
    PortError::new("stored certificates", what)
}

fn method_columns(method: &ChallengeMethod) -> (&'static str, Option<String>, String) {
    match method {
        ChallengeMethod::Dns { provider, values } => {
            ("dns", Some(provider.clone()), serde_json::to_string(values).unwrap_or_else(|_| "{}".into()))
        }
        ChallengeMethod::Http => ("http", None, "{}".into()),
    }
}

fn method(kind: &str, provider: Option<String>, credentials: &str) -> Result<ChallengeMethod, PortError> {
    match (kind, provider) {
        ("dns", Some(provider)) => {
            let values: BTreeMap<String, String> =
                serde_json::from_str(credentials).map_err(|_| stored("a challenge's values"))?;
            Ok(ChallengeMethod::Dns { provider, values })
        }
        ("http", None) => Ok(ChallengeMethod::Http),
        _ => Err(stored("a challenge's method")),
    }
}

fn port(value: i64) -> Result<u16, PortError> {
    u16::try_from(value).map_err(|_| stored("a port"))
}

#[async_trait]
impl CertificateRepository for SqliteCertificates {
    async fn accounts(&self) -> Result<Vec<AcmeAccount>, PortError> {
        let rows = sqlx::query!("SELECT id, name, server, email, created_at FROM acme_accounts ORDER BY id")
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(AcmeAccount {
                    id: row.id,
                    name: row.name,
                    server: AcmeServer::parse(&row.server).ok_or_else(|| stored("an account's server"))?,
                    email: row.email,
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                })
            })
            .collect()
    }

    async fn create_account(&self, account: &NewAccount, now: Timestamp) -> Result<AcmeAccount, PortError> {
        let (server, at) = (account.server.as_str(), now.unix_seconds());
        let created = sqlx::query!(
            "INSERT INTO acme_accounts (name, server, email, created_at) VALUES (?, ?, ?, ?)",
            account.name,
            server,
            account.email,
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(AcmeAccount {
            id: created.last_insert_rowid(),
            name: account.name.clone(),
            server: account.server,
            email: account.email.clone(),
            created_at: now,
        })
    }

    async fn update_account(&self, id: i64, account: &NewAccount) -> Result<bool, PortError> {
        let server = account.server.as_str();
        let changed = sqlx::query!(
            "UPDATE acme_accounts SET name = ?, server = ?, email = ? WHERE id = ?",
            account.name,
            server,
            account.email,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(changed.rows_affected() > 0)
    }

    async fn delete_account(&self, id: i64) -> Result<bool, PortError> {
        let deleted = sqlx::query!("DELETE FROM acme_accounts WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn challenges(&self) -> Result<Vec<Challenge>, PortError> {
        let rows = sqlx::query!(
            "SELECT id, name, method, provider, credentials, created_at FROM acme_challenges ORDER BY id"
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(Challenge {
                    id: row.id,
                    name: row.name,
                    method: method(&row.method, row.provider, &row.credentials)?,
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                })
            })
            .collect()
    }

    async fn create_challenge(
        &self,
        name: &str,
        method: &ChallengeMethod,
        now: Timestamp,
    ) -> Result<Challenge, PortError> {
        let (kind, provider, credentials) = method_columns(method);
        let at = now.unix_seconds();
        let created = sqlx::query!(
            "INSERT INTO acme_challenges (name, method, provider, credentials, created_at) VALUES (?, ?, ?, ?, ?)",
            name,
            kind,
            provider,
            credentials,
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(Challenge {
            id: created.last_insert_rowid(),
            name: name.to_owned(),
            method: method.clone(),
            created_at: now,
        })
    }

    async fn update_challenge(
        &self,
        id: i64,
        name: &str,
        method: &ChallengeMethod,
    ) -> Result<bool, PortError> {
        let (kind, provider, credentials) = method_columns(method);
        let changed = sqlx::query!(
            "UPDATE acme_challenges SET name = ?, method = ?, provider = ?, credentials = ? WHERE id = ?",
            name,
            kind,
            provider,
            credentials,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(changed.rows_affected() > 0)
    }

    async fn delete_challenge(&self, id: i64) -> Result<bool, PortError> {
        let deleted = sqlx::query!("DELETE FROM acme_challenges WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn certificates(&self) -> Result<Vec<Certificate>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT id, domains, key_type, account_id, challenge_id, auto_renew AS "auto_renew: bool",
                      created_at, issued_at, not_after, last_attempt_at, last_error, last_output
               FROM certificates ORDER BY id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                Ok(Certificate {
                    id: row.id,
                    domains: serde_json::from_str(&row.domains)
                        .map_err(|_| stored("a certificate's names"))?,
                    key: KeyType::parse(&row.key_type).ok_or_else(|| stored("a certificate's key"))?,
                    account_id: row.account_id,
                    challenge_id: row.challenge_id,
                    auto_renew: row.auto_renew,
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                    issued_at: row.issued_at.map(Timestamp::from_unix_seconds),
                    not_after: row.not_after.map(Timestamp::from_unix_seconds),
                    last_attempt_at: row.last_attempt_at.map(Timestamp::from_unix_seconds),
                    last_error: row.last_error,
                    last_output: serde_json::from_str(&row.last_output).unwrap_or_default(),
                })
            })
            .collect()
    }

    async fn create_certificate(
        &self,
        certificate: &NewCertificate,
        now: Timestamp,
    ) -> Result<Certificate, PortError> {
        let domains = serde_json::to_string(&certificate.domains).unwrap_or_else(|_| "[]".into());
        let (key, at) = (certificate.key.as_str(), now.unix_seconds());
        let created = sqlx::query!(
            "INSERT INTO certificates (domains, key_type, account_id, challenge_id, auto_renew, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            domains,
            key,
            certificate.account_id,
            certificate.challenge_id,
            certificate.auto_renew,
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(Certificate {
            id: created.last_insert_rowid(),
            domains: certificate.domains.clone(),
            key: certificate.key,
            account_id: certificate.account_id,
            challenge_id: certificate.challenge_id,
            auto_renew: certificate.auto_renew,
            created_at: now,
            issued_at: None,
            not_after: None,
            last_attempt_at: None,
            last_error: None,
            last_output: Vec::new(),
        })
    }

    async fn update_certificate(&self, id: i64, certificate: &NewCertificate) -> Result<bool, PortError> {
        let domains = serde_json::to_string(&certificate.domains).unwrap_or_else(|_| "[]".into());
        let key = certificate.key.as_str();
        let changed = sqlx::query!(
            "UPDATE certificates SET domains = ?, key_type = ?, account_id = ?, challenge_id = ?, auto_renew = ?
             WHERE id = ?",
            domains,
            key,
            certificate.account_id,
            certificate.challenge_id,
            certificate.auto_renew,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(changed.rows_affected() > 0)
    }

    async fn delete_certificate(&self, id: i64) -> Result<bool, PortError> {
        let deleted = sqlx::query!("DELETE FROM certificates WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn record_attempt(&self, id: i64, attempt: &Attempt) -> Result<bool, PortError> {
        let at = attempt.at.unix_seconds();
        let output = serde_json::to_string(&attempt.output).unwrap_or_else(|_| "[]".into());
        let changed = match &attempt.outcome {
            Ok(not_after) => {
                let not_after = not_after.unix_seconds();
                sqlx::query!(
                    "UPDATE certificates SET issued_at = ?, not_after = ?, last_attempt_at = ?, last_error = NULL,
                         last_output = ?
                     WHERE id = ?",
                    at,
                    not_after,
                    at,
                    output,
                    id
                )
                .execute(self.db.pool())
                .await
            }
            Err(reason) => {
                sqlx::query!(
                    "UPDATE certificates SET last_attempt_at = ?, last_error = ?, last_output = ? WHERE id = ?",
                    at,
                    reason,
                    output,
                    id
                )
                .execute(self.db.pool())
                .await
            }
        }
        .map_err(db_error)?;
        Ok(changed.rows_affected() > 0)
    }

    async fn web_server(&self) -> Result<WebServer, PortError> {
        let row = sqlx::query!("SELECT http_port, https_port, certificate_id FROM web_server WHERE id = 1")
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        let Some(row) = row else { return Ok(WebServer::default()) };
        Ok(WebServer {
            http_port: row.http_port.map(port).transpose()?,
            https_port: port(row.https_port)?,
            certificate_id: row.certificate_id,
        })
    }

    async fn save_web_server(&self, web: &WebServer) -> Result<(), PortError> {
        let (http, https) = (web.http_port.map(i64::from), i64::from(web.https_port));
        sqlx::query!(
            "INSERT INTO web_server (id, http_port, https_port, certificate_id) VALUES (1, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET http_port = excluded.http_port, https_port = excluded.https_port,
                 certificate_id = excluded.certificate_id",
            http,
            https,
            web.certificate_id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    #[tokio::test]
    async fn keeps_accounts_challenges_certificates_and_the_web_server() {
        let (_dir, db) = temporary_database().await;
        let repository = SqliteCertificates::new(db);
        let now = Timestamp::from_unix_seconds(1_793_880_000);
        let new_account = NewAccount {
            name: "LE".into(),
            server: AcmeServer::LetsEncryptStaging,
            email: "a@example.org".into(),
        };
        let account = repository.create_account(&new_account, now).await.unwrap();
        assert_eq!(repository.accounts().await.unwrap(), std::slice::from_ref(&account));
        let production = NewAccount { server: AcmeServer::LetsEncrypt, ..new_account };
        assert!(repository.update_account(account.id, &production).await.unwrap());
        assert_eq!(repository.accounts().await.unwrap()[0].server, AcmeServer::LetsEncrypt);

        let values = BTreeMap::from([
            ("IONOS_PREFIX".to_owned(), "p".to_owned()),
            ("IONOS_SECRET".into(), "s".into()),
        ]);
        let dns = ChallengeMethod::Dns { provider: "dns_ionos".into(), values };
        let challenge = repository.create_challenge("IONOS", &dns, now).await.unwrap();
        assert_eq!(repository.challenges().await.unwrap(), std::slice::from_ref(&challenge));
        assert!(repository.update_challenge(challenge.id, "HTTP", &ChallengeMethod::Http).await.unwrap());
        assert_eq!(repository.challenges().await.unwrap()[0].method, ChallengeMethod::Http);
        assert!(repository.update_challenge(challenge.id, "IONOS", &dns).await.unwrap());

        let new = NewCertificate {
            domains: vec!["*.example.org".into(), "example.org".into()],
            key: KeyType::Rsa4096,
            account_id: account.id,
            challenge_id: challenge.id,
            auto_renew: true,
        };
        let certificate = repository.create_certificate(&new, now).await.unwrap();
        assert_eq!(repository.certificates().await.unwrap(), std::slice::from_ref(&certificate));
        let until = Timestamp::from_unix_seconds(1_801_656_000);
        let issued = Attempt { at: now, outcome: Ok(until), output: vec!["Cert success.".into()] };
        assert!(repository.record_attempt(certificate.id, &issued).await.unwrap());
        let failed = Attempt { at: now, outcome: Err("DNS API: unauthorized".into()), output: vec![] };
        assert!(repository.record_attempt(certificate.id, &failed).await.unwrap());
        let kept = repository.certificates().await.unwrap().remove(0);
        assert_eq!((kept.issued_at, kept.not_after), (Some(now), Some(until)), "the issued one still holds");
        assert_eq!(kept.last_error.as_deref(), Some("DNS API: unauthorized"));
        assert!(kept.last_output.is_empty());
        let fewer = NewCertificate { domains: vec!["nas.example.org".into()], auto_renew: false, ..new };
        assert!(repository.update_certificate(certificate.id, &fewer).await.unwrap());
        assert_eq!(repository.certificates().await.unwrap()[0].domains, ["nas.example.org"]);

        assert_eq!(repository.web_server().await.unwrap(), WebServer::default());
        let web = WebServer { http_port: None, https_port: 8443, certificate_id: Some(certificate.id) };
        repository.save_web_server(&web).await.unwrap();
        assert_eq!(repository.web_server().await.unwrap(), web);
        // Shown by the web interface: not removed under it.
        assert!(repository.delete_certificate(certificate.id).await.is_err());
        repository.save_web_server(&WebServer::default()).await.unwrap();
        assert!(repository.delete_certificate(certificate.id).await.unwrap());
        assert!(repository.delete_challenge(challenge.id).await.unwrap());
        assert!(repository.delete_account(account.id).await.unwrap());
        assert!(!repository.delete_account(account.id).await.unwrap());
        assert!(!repository.update_certificate(certificate.id, &fewer).await.unwrap());
    }
}
