use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_ups::application::{SettingsStore, Stored};
use zendiloos_ups::domain::{Account, Settings, ShutdownWhen, SnmpVersion, Source};

use super::Database;

/// [`SettingsStore`] on the `ups_settings` table.
#[derive(Debug, Clone)]
pub struct SqliteUpsSettings {
    db: Database,
}

impl SqliteUpsSettings {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("UPS settings", error)
}

/// The settings as the `settings` column keeps them.
#[derive(Serialize, Deserialize, Default)]
struct Document {
    source: Option<SourceDocument>,
    #[serde(default)]
    shutdown: ShutdownDocument,
    #[serde(default)]
    cut_power: bool,
    #[serde(default)]
    serve: Option<AccountDocument>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SourceDocument {
    Usb { driver: String, vendor_id: Option<String>, product_id: Option<String>, serial: Option<String> },
    Snmp { address: String, community: String, version: String },
    Server { host: String, port: u16, ups: String, user: String, password: String },
    Simulated,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShutdownDocument {
    #[default]
    LowBattery,
    ChargeBelow {
        percent: u8,
    },
    AfterMinutes {
        minutes: u16,
    },
}

#[derive(Serialize, Deserialize)]
struct AccountDocument {
    user: String,
    password: String,
}

fn document(settings: &Settings) -> Document {
    Document {
        source: settings.source.as_ref().map(|source| match source.clone() {
            Source::Usb { driver, vendor_id, product_id, serial } => {
                SourceDocument::Usb { driver, vendor_id, product_id, serial }
            }
            Source::Snmp { address, community, version } => SourceDocument::Snmp {
                address,
                community,
                version: match version {
                    SnmpVersion::V1 => "v1".into(),
                    SnmpVersion::V2c => "v2c".into(),
                },
            },
            Source::Server { host, port, ups, user, password } => {
                SourceDocument::Server { host, port, ups, user, password }
            }
            Source::Simulated => SourceDocument::Simulated,
        }),
        shutdown: match settings.shutdown {
            ShutdownWhen::LowBattery => ShutdownDocument::LowBattery,
            ShutdownWhen::ChargeBelow(percent) => ShutdownDocument::ChargeBelow { percent },
            ShutdownWhen::AfterMinutes(minutes) => ShutdownDocument::AfterMinutes { minutes },
        },
        cut_power: settings.cut_power,
        serve: settings.serve.as_ref().map(|account| AccountDocument {
            user: account.user.clone(),
            password: account.password.clone(),
        }),
    }
}

fn settings(document: Document) -> Settings {
    Settings {
        source: document.source.map(|source| match source {
            SourceDocument::Usb { driver, vendor_id, product_id, serial } => {
                Source::Usb { driver, vendor_id, product_id, serial }
            }
            SourceDocument::Snmp { address, community, version } => Source::Snmp {
                address,
                community,
                version: if version == "v1" { SnmpVersion::V1 } else { SnmpVersion::V2c },
            },
            SourceDocument::Server { host, port, ups, user, password } => {
                Source::Server { host, port, ups, user, password }
            }
            SourceDocument::Simulated => Source::Simulated,
        }),
        shutdown: match document.shutdown {
            ShutdownDocument::LowBattery => ShutdownWhen::LowBattery,
            ShutdownDocument::ChargeBelow { percent } => ShutdownWhen::ChargeBelow(percent),
            ShutdownDocument::AfterMinutes { minutes } => ShutdownWhen::AfterMinutes(minutes),
        },
        cut_power: document.cut_power,
        serve: document.serve.map(|account| Account { user: account.user, password: account.password }),
    }
}

/// A password for the NAS's own account at NUT's server: 32 hexadecimal
/// digits, which is a word NUT's files take as it is.
fn new_password() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encoded(settings: &Settings) -> Result<String, PortError> {
    serde_json::to_string(&document(settings)).map_err(|error| PortError::new("UPS settings", error))
}

#[async_trait]
impl SettingsStore for SqliteUpsSettings {
    async fn load(&self) -> Result<Stored, PortError> {
        let row = sqlx::query!("SELECT settings, local_password FROM ups_settings WHERE id = 1")
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?;
        if let Some(row) = row {
            let document: Document =
                serde_json::from_str(&row.settings).map_err(|error| PortError::new("UPS settings", error))?;
            return Ok(Stored { settings: settings(document), local_password: row.local_password });
        }
        // The first look makes the row, and with it the NAS's own password.
        let (none, password) = (encoded(&Settings::default())?, new_password());
        sqlx::query!(
            "INSERT INTO ups_settings (id, settings, local_password, updated_at) VALUES (1, ?, ?, 0)
             ON CONFLICT (id) DO NOTHING",
            none,
            password
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        let row = sqlx::query!("SELECT settings, local_password FROM ups_settings WHERE id = 1")
            .fetch_one(self.db.pool())
            .await
            .map_err(db_error)?;
        let document: Document =
            serde_json::from_str(&row.settings).map_err(|error| PortError::new("UPS settings", error))?;
        Ok(Stored { settings: settings(document), local_password: row.local_password })
    }

    async fn save(&self, settings: &Settings, at: Timestamp) -> Result<(), PortError> {
        let (document, password, at) = (encoded(settings)?, new_password(), at.unix_seconds());
        // The password is for a row that is not there yet; one that is keeps its own.
        sqlx::query!(
            "INSERT INTO ups_settings (id, settings, local_password, updated_at) VALUES (1, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET settings = excluded.settings, updated_at = excluded.updated_at",
            document,
            password,
            at
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

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    #[tokio::test]
    async fn the_first_look_makes_the_nas_s_own_password_which_then_stays() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteUpsSettings::new(db);
        let first = store.load().await.unwrap();
        assert_eq!(first.settings, Settings::default());
        assert_eq!(first.local_password.len(), 32);
        assert!(first.local_password.chars().all(|c| c.is_ascii_hexdigit()));

        let server = Settings {
            source: Some(Source::Server {
                host: "nas.local".into(),
                port: 3493,
                ups: "ups".into(),
                user: "monuser".into(),
                password: "secret".into(),
            }),
            shutdown: ShutdownWhen::AfterMinutes(10),
            cut_power: false,
            serve: None,
        };
        store.save(&server, at(100)).await.unwrap();
        let kept = store.load().await.unwrap();
        assert_eq!(kept.settings, server);
        assert_eq!(kept.local_password, first.local_password, "saving keeps it");
    }

    #[tokio::test]
    async fn every_kind_of_ups_comes_back_as_it_was_saved() {
        let (_dir, db) = temporary_database().await;
        let store = SqliteUpsSettings::new(db);
        let serve = Some(Account { user: "monuser".into(), password: "for-others".into() });
        for settings in [
            Settings {
                source: Some(Source::Usb {
                    driver: "usbhid-ups".into(),
                    vendor_id: Some("0463".into()),
                    product_id: Some("ffff".into()),
                    serial: None,
                }),
                shutdown: ShutdownWhen::ChargeBelow(30),
                cut_power: true,
                serve: serve.clone(),
            },
            Settings {
                source: Some(Source::Snmp {
                    address: "10.0.0.9".into(),
                    community: "public".into(),
                    version: SnmpVersion::V1,
                }),
                ..Settings::default()
            },
            Settings { source: Some(Source::Simulated), serve, ..Settings::default() },
            Settings::default(),
        ] {
            store.save(&settings, at(1)).await.unwrap();
            assert_eq!(store.load().await.unwrap().settings, settings);
        }
        // Saved before anything was read, the row gets its password too.
        let (_other, db) = temporary_database().await;
        let fresh = SqliteUpsSettings::new(db);
        fresh.save(&Settings::default(), at(1)).await.unwrap();
        assert_eq!(fresh.load().await.unwrap().local_password.len(), 32);
    }
}
