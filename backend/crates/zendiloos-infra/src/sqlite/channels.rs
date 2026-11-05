use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_notify::Level;
use zendiloos_notify::channels::{
    Channel, ChannelRepository, ChannelSettings, Delivery, Email, Saved, Security, Target, Webhook,
    WebhookFormat,
};
use zendiloos_notify::message::Language;

use super::Database;

/// [`ChannelRepository`] on the `notification_channels` table.
#[derive(Debug, Clone)]
pub struct SqliteChannels {
    db: Database,
}

impl SqliteChannels {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("notification channels", error)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Stored {
    Email {
        server: String,
        port: u16,
        security: String,
        username: Option<String>,
        password: Option<String>,
        from: String,
        to: Vec<String>,
    },
    Webhook {
        url: String,
        format: String,
    },
}

fn stored(target: &Target) -> (&'static str, String) {
    let value = match target {
        Target::Email(e) => Stored::Email {
            server: e.server.clone(),
            port: e.port,
            security: e.security.as_str().to_owned(),
            username: e.username.clone(),
            password: e.password.clone(),
            from: e.from.clone(),
            to: e.to.clone(),
        },
        Target::Webhook(w) => Stored::Webhook { url: w.url.clone(), format: w.format.as_str().to_owned() },
    };
    let kind = if matches!(target, Target::Email(_)) { "email" } else { "webhook" };
    (kind, serde_json::to_string(&value).unwrap_or_default())
}

fn target(json: &str) -> Option<Target> {
    Some(match serde_json::from_str(json).ok()? {
        Stored::Email { server, port, security, username, password, from, to } => Target::Email(Email {
            server,
            port,
            security: Security::parse(&security)?,
            username,
            password,
            from,
            to,
        }),
        Stored::Webhook { url, format } => {
            Target::Webhook(Webhook { url, format: WebhookFormat::parse(&format)? })
        }
    })
}

#[async_trait]
impl ChannelRepository for SqliteChannels {
    async fn list(&self) -> Result<Vec<Channel>, PortError> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", name, settings, min_level, language, last_at, last_error
               FROM notification_channels ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                // A row this version cannot read is left out, not guessed at.
                let settings = ChannelSettings {
                    name: row.name,
                    target: target(&row.settings)?,
                    min_level: Level::parse(&row.min_level)?,
                    language: Language::parse(&row.language)?,
                };
                let last = row
                    .last_at
                    .map(|at| Delivery { at: Timestamp::from_unix_seconds(at), error: row.last_error });
                Some(Channel { id: row.id, settings, last })
            })
            .collect())
    }

    async fn create(&self, settings: &ChannelSettings, at: Timestamp) -> Result<Option<Channel>, PortError> {
        let (kind, json) = stored(&settings.target);
        let (level, language, at) =
            (settings.min_level.as_str(), settings.language.as_str(), at.unix_seconds());
        let created = sqlx::query!(
            "INSERT INTO notification_channels (name, kind, settings, min_level, language, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (name) DO NOTHING",
            settings.name,
            kind,
            json,
            level,
            language,
            at,
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        if created.rows_affected() == 0 {
            return Ok(None);
        }
        let id = created.last_insert_rowid();
        Ok(Some(Channel { id, settings: settings.clone(), last: None }))
    }

    async fn update(&self, id: i64, settings: &ChannelSettings, at: Timestamp) -> Result<Saved, PortError> {
        let taken = sqlx::query_scalar!(
            "SELECT id FROM notification_channels WHERE name = ? AND id != ?",
            settings.name,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        if taken.is_some() {
            return Ok(Saved::NameTaken);
        }
        let (kind, json) = stored(&settings.target);
        let (level, language, at) =
            (settings.min_level.as_str(), settings.language.as_str(), at.unix_seconds());
        let updated = sqlx::query!(
            "UPDATE notification_channels SET name = ?, kind = ?, settings = ?, min_level = ?, language = ?,
             updated_at = ? WHERE id = ?",
            settings.name,
            kind,
            json,
            level,
            language,
            at,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(if updated.rows_affected() == 0 { Saved::NotFound } else { Saved::Done })
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let deleted = sqlx::query!("DELETE FROM notification_channels WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn delivered(&self, id: i64, delivery: &Delivery) -> Result<(), PortError> {
        let at = delivery.at.unix_seconds();
        sqlx::query!(
            "UPDATE notification_channels SET last_at = ?, last_error = ? WHERE id = ?",
            at,
            delivery.error,
            id
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

    fn mail() -> ChannelSettings {
        ChannelSettings {
            name: "Admins".into(),
            target: Target::Email(Email {
                server: "mail.example.org".into(),
                port: 465,
                security: Security::Tls,
                username: Some("nas".into()),
                password: Some("secret".into()),
                from: "nas@example.org".into(),
                to: vec!["a@example.org".into(), "b@example.org".into()],
            }),
            min_level: Level::Warning,
            language: Language::German,
        }
    }

    #[tokio::test]
    async fn channels_keep_their_settings_and_last_delivery() {
        let (_dir, db) = temporary_database().await;
        let channels = SqliteChannels::new(db.clone());
        let at = Timestamp::from_unix_seconds(1_793_836_800);

        let created = channels.create(&mail(), at).await.unwrap().unwrap();
        assert!(channels.create(&mail(), at).await.unwrap().is_none(), "name taken");
        let hook = ChannelSettings {
            name: "Chat".into(),
            target: Target::Webhook(Webhook {
                url: "https://x.example/h".into(),
                format: WebhookFormat::Text,
            }),
            min_level: Level::Error,
            language: Language::English,
        };
        let chat = channels.create(&hook, at).await.unwrap().unwrap();
        assert_eq!(channels.list().await.unwrap(), [created.clone(), chat.clone()]);

        let failed = Delivery { at, error: Some("refused".into()) };
        channels.delivered(chat.id, &failed).await.unwrap();
        let renamed = ChannelSettings { name: "Admins".into(), ..hook };
        assert_eq!(channels.update(chat.id, &renamed, at).await.unwrap(), Saved::NameTaken);
        let other = ChannelSettings { name: "Other".into(), ..mail() };
        assert_eq!(channels.update(99, &other, at).await.unwrap(), Saved::NotFound);
        let listed = channels.list().await.unwrap();
        assert_eq!(listed[1].last, Some(failed));

        assert!(channels.delete(created.id).await.unwrap());
        assert!(!channels.delete(created.id).await.unwrap());
        db.close().await;
    }
}
