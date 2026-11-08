use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_notify::Level;
use zendiloos_notify::channels::{
    AccountAddress, AccountAddresses, Channel, ChannelRepository, ChannelSettings, Delivery, Destination,
    MailServer, Recipient, Saved, Security, Webhook, WebhookFormat,
};
use zendiloos_notify::message::Language;
use zendiloos_notify::routes::{RouteRepository, Routes};

use super::Database;

/// [`ChannelRepository`] on the `notification_channels` and
/// `notification_mail_server` tables, and [`RouteRepository`] on
/// `notification_routes`.
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

/// A channel's destination as the `settings` column keeps it: an email
/// channel's addresses (`to`) and accounts, or a webhook.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Stored {
    Email {
        #[serde(default)]
        to: Vec<String>,
        #[serde(default)]
        accounts: Vec<i64>,
    },
    Webhook {
        url: String,
        format: String,
    },
}

fn stored(destination: &Destination) -> (&'static str, String) {
    let (kind, value) = match destination {
        Destination::Email { recipients } => {
            let (mut to, mut accounts) = (Vec::new(), Vec::new());
            for recipient in recipients {
                match recipient {
                    Recipient::Account(id) => accounts.push(*id),
                    Recipient::Address(address) => to.push(address.clone()),
                }
            }
            ("email", Stored::Email { to, accounts })
        }
        Destination::Webhook(w) => {
            ("webhook", Stored::Webhook { url: w.url.clone(), format: w.format.as_str().to_owned() })
        }
    };
    (kind, serde_json::to_string(&value).unwrap_or_default())
}

/// The accounts first, then the addresses.
fn destination(json: &str) -> Option<Destination> {
    Some(match serde_json::from_str(json).ok()? {
        Stored::Email { to, accounts } => Destination::Email {
            recipients: accounts
                .into_iter()
                .map(Recipient::Account)
                .chain(to.into_iter().map(Recipient::Address))
                .collect(),
        },
        Stored::Webhook { url, format } => {
            Destination::Webhook(Webhook { url, format: WebhookFormat::parse(&format)? })
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
                    destination: destination(&row.settings)?,
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
        let (kind, json) = stored(&settings.destination);
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
        let (kind, json) = stored(&settings.destination);
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
        // Its routes go with it (ON DELETE CASCADE).
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

    async fn mail_server(&self) -> Result<Option<MailServer>, PortError> {
        let row = sqlx::query!(
            r#"SELECT server, port AS "port!: i64", security, username, password, sender
               FROM notification_mail_server WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(row.and_then(|row| {
            Some(MailServer {
                server: row.server,
                port: u16::try_from(row.port).ok()?,
                security: Security::parse(&row.security)?,
                username: row.username,
                password: row.password,
                from: row.sender,
            })
        }))
    }

    async fn set_mail_server(&self, server: &MailServer, at: Timestamp) -> Result<(), PortError> {
        let (port, security, at) = (i64::from(server.port), server.security.as_str(), at.unix_seconds());
        sqlx::query!(
            "INSERT INTO notification_mail_server (id, server, port, security, username, password, sender, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET server = excluded.server, port = excluded.port,
                 security = excluded.security, username = excluded.username, password = excluded.password,
                 sender = excluded.sender, updated_at = excluded.updated_at",
            server.server,
            port,
            security,
            server.username,
            server.password,
            server.from,
            at
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[async_trait]
impl RouteRepository for SqliteChannels {
    async fn routes(&self) -> Result<Routes, PortError> {
        let rows =
            sqlx::query!(r#"SELECT kind, channel_id, enabled AS "enabled: bool" FROM notification_routes"#)
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        let mut routes = Routes::default();
        for row in rows {
            match row.channel_id {
                None => {
                    routes.in_app.insert(row.kind, row.enabled);
                }
                Some(channel) => {
                    routes.channels.insert((channel, row.kind), row.enabled);
                }
            }
        }
        Ok(routes)
    }

    async fn replace(&self, routes: &Routes) -> Result<(), PortError> {
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM notification_routes").execute(&mut *tx).await.map_err(db_error)?;
        let rows =
            routes.in_app.iter().map(|(kind, enabled)| (kind, None, *enabled)).chain(
                routes.channels.iter().map(|((channel, kind), enabled)| (kind, Some(*channel), *enabled)),
            );
        for (kind, channel, enabled) in rows {
            sqlx::query!(
                "INSERT INTO notification_routes (kind, channel_id, enabled)
                 SELECT ?1, ?2, ?3 WHERE ?2 IS NULL OR EXISTS (SELECT 1 FROM notification_channels WHERE id = ?2)",
                kind,
                channel,
                enabled
            )
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)
    }
}

/// [`AccountAddresses`] on the `accounts` table.
#[derive(Debug, Clone)]
pub struct SqliteAccountAddresses {
    db: Database,
}

impl SqliteAccountAddresses {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AccountAddresses for SqliteAccountAddresses {
    async fn address(&self, account_id: i64) -> Result<AccountAddress, PortError> {
        let row = sqlx::query!(
            r#"SELECT email, disabled AS "disabled: bool" FROM accounts WHERE id = ?"#,
            account_id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| PortError::new("accounts", e))?;
        Ok(match row {
            None => AccountAddress::Gone,
            Some(row) if row.disabled => AccountAddress::Disabled,
            Some(row) => {
                row.email.filter(|e| !e.is_empty()).map_or(AccountAddress::Missing, AccountAddress::Address)
            }
        })
    }
}

/// The routes in a form that compares well in tests.
#[cfg(test)]
fn by_target(routes: &Routes) -> std::collections::BTreeMap<(i64, String), bool> {
    routes
        .in_app
        .iter()
        .map(|(kind, on)| ((0, kind.clone()), *on))
        .chain(routes.channels.iter().map(|(key, on)| (key.clone(), *on)))
        .collect()
}

#[cfg(test)]
mod tests {
    use zendiloos_notify::routes::KINDS;

    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn mail(name: &str, recipients: Vec<Recipient>) -> ChannelSettings {
        ChannelSettings {
            name: name.into(),
            destination: Destination::Email { recipients },
            min_level: Level::Warning,
            language: Language::German,
        }
    }

    fn server() -> MailServer {
        MailServer {
            server: "mail.example.org".into(),
            port: 465,
            security: Security::Tls,
            username: Some("nas".into()),
            password: Some("secret".into()),
            from: "nas@example.org".into(),
        }
    }

    #[tokio::test]
    async fn channels_keep_their_recipients_and_last_delivery() {
        let (_dir, db) = temporary_database().await;
        let channels = SqliteChannels::new(db.clone());
        let at = Timestamp::from_unix_seconds(1_793_836_800);

        let recipients = vec![Recipient::Account(3), Recipient::Address("a@example.org".into())];
        let created = channels.create(&mail("Admins", recipients), at).await.unwrap().unwrap();
        assert!(channels.create(&mail("Admins", vec![]), at).await.unwrap().is_none(), "name taken");
        let hook = ChannelSettings {
            name: "Chat".into(),
            destination: Destination::Webhook(Webhook {
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
        let other = mail("Other", vec![]);
        assert_eq!(channels.update(99, &other, at).await.unwrap(), Saved::NotFound);
        let listed = channels.list().await.unwrap();
        assert_eq!(listed[1].last, Some(failed));

        assert_eq!(channels.mail_server().await.unwrap(), None);
        channels.set_mail_server(&server(), at).await.unwrap();
        let mut changed = server();
        (changed.port, changed.security, changed.password) = (587, Security::StartTls, None);
        channels.set_mail_server(&changed, at).await.unwrap();
        assert_eq!(channels.mail_server().await.unwrap(), Some(changed));

        let mut routes = Routes::default();
        routes.in_app.insert("scrub_finished".into(), false);
        routes.channels.insert((chat.id, "pool_trouble".into()), true);
        routes.channels.insert((99, "pool_trouble".into()), true);
        channels.replace(&routes).await.unwrap();
        routes.channels.remove(&(99, "pool_trouble".into()));
        assert_eq!(by_target(&channels.routes().await.unwrap()), by_target(&routes), "no routes to nowhere");

        assert!(channels.delete(chat.id).await.unwrap());
        assert!(!channels.delete(chat.id).await.unwrap());
        assert!(channels.routes().await.unwrap().channels.is_empty(), "its routes went with it");
        db.close().await;
    }

    #[tokio::test]
    async fn an_accounts_address_is_read_as_it_is_now() {
        let (_dir, db) = temporary_database().await;
        let addresses = SqliteAccountAddresses::new(db.clone());
        for (name, email, disabled) in
            [("anna", Some("anna@example.org"), 0), ("ben", None, 0), ("carl", Some("c@example.org"), 1)]
        {
            sqlx::query("INSERT INTO accounts (name, role, password_hash, email, disabled, created_at, updated_at) VALUES (?, 'user', 'x', ?, ?, 0, 0)")
                .bind(name)
                .bind(email)
                .bind(disabled)
                .execute(db.pool())
                .await
                .unwrap();
        }
        let id = |name: &str| {
            let db = db.clone();
            let name = name.to_owned();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE name = ?")
                    .bind(name)
                    .fetch_one(db.pool())
                    .await
                    .unwrap()
            }
        };
        assert_eq!(
            addresses.address(id("anna").await).await.unwrap(),
            AccountAddress::Address("anna@example.org".into())
        );
        assert_eq!(addresses.address(id("ben").await).await.unwrap(), AccountAddress::Missing);
        assert_eq!(addresses.address(id("carl").await).await.unwrap(), AccountAddress::Disabled);
        assert_eq!(addresses.address(999).await.unwrap(), AccountAddress::Gone);
        db.close().await;
    }

    /// A channel as the version before routes kept it becomes the mail
    /// server, an email channel to its recipients, and routes that carry
    /// what it carried.
    #[tokio::test]
    async fn the_channels_of_before_carry_on_as_they_did() {
        let (_dir, db) = temporary_database().await;
        let old = r#"{"kind":"email","server":"smtp.ionos.de","port":587,"security":"starttls",
            "username":"julian","password":"secret","from":"nas@example.org","to":["julian@example.org"]}"#;
        for (name, kind, settings, level) in [
            ("ionos", "email", old, "warning"),
            ("chat", "webhook", r#"{"kind":"webhook","url":"https://x.example","format":"json"}"#, "error"),
        ] {
            sqlx::query(
                "INSERT INTO notification_channels (name, kind, settings, min_level, language, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'de', 1, 2)",
            )
            .bind(name)
            .bind(kind)
            .bind(settings)
            .bind(level)
            .execute(db.pool())
            .await
            .unwrap();
        }
        // What 0032 does to the rows it finds (its tables exist already).
        let migration = include_str!("../../migrations/0032_notification_routes.sql");
        let data: Vec<&str> = migration
            .split(';')
            .map(|statement| {
                statement.lines().filter(|l| !l.trim_start().starts_with("--")).collect::<Vec<_>>()
            })
            .map(|lines| lines.join("\n"))
            .filter(|statement| {
                let statement = statement.trim_start();
                statement.starts_with("INSERT") || statement.starts_with("UPDATE")
            })
            .map(|statement| Box::leak(statement.into_boxed_str()) as &str)
            .collect();
        assert_eq!(data.len(), 2, "the server, then the recipients");
        for statement in data {
            sqlx::query(statement).execute(db.pool()).await.unwrap();
        }

        let channels = SqliteChannels::new(db.clone());
        assert_eq!(
            channels.mail_server().await.unwrap(),
            Some(MailServer {
                server: "smtp.ionos.de".into(),
                port: 587,
                security: Security::StartTls,
                username: Some("julian".into()),
                password: Some("secret".into()),
                from: "nas@example.org".into(),
            })
        );
        let listed = channels.list().await.unwrap();
        let ionos = listed.iter().find(|c| c.settings.name == "ionos").unwrap();
        assert_eq!(
            ionos.settings.destination,
            Destination::Email { recipients: vec![Recipient::Address("julian@example.org".into())] }
        );
        let chat = listed.iter().find(|c| c.settings.name == "chat").unwrap();
        let routes = channels.routes().await.unwrap();
        assert_eq!(routes, Routes::default(), "nothing chosen yet");
        for kind in KINDS {
            assert!(routes.in_app(kind.key));
            let carried = |channel: &Channel| {
                routes.to_channel(channel.id, channel.settings.min_level, kind.key, kind.level)
            };
            assert_eq!(carried(ionos), kind.level != Level::Info, "{}", kind.key);
            assert_eq!(carried(chat), kind.level == Level::Error, "{}", kind.key);
        }
        // The chat took errors only: it gets every kind that can be one,
        // a faulted pool among them, and nothing that never is.
        let errors: Vec<&str> = KINDS
            .iter()
            .filter(|kind| routes.to_channel(chat.id, chat.settings.min_level, kind.key, kind.level))
            .map(|kind| kind.key)
            .collect();
        assert_eq!(
            errors,
            [
                "pool_trouble",
                "disk_health",
                "volume_filling",
                "updates_installed",
                "updates_failed",
                "certificate_renewal_failed",
                "docker_backup_failed"
            ]
        );
        db.close().await;
    }
}
