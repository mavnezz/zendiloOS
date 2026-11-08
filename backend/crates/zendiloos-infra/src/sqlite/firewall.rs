//! The firewall's settings and rules, and the services told blocked.

use std::collections::BTreeSet;

use async_trait::async_trait;
use zendiloos_firewall::application::FirewallRepository;
use zendiloos_firewall::domain::{
    Action, FirewallSettings, KnownService, Protocol, Rule, Service, Source, parse_ports, ports_text,
};
use zendiloos_kernel::PortError;

use super::Database;

/// `firewall_settings`, `firewall_rules` and `firewall_blocked`.
#[derive(Debug, Clone)]
pub struct SqliteFirewall {
    db: Database,
}

impl SqliteFirewall {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("firewall", error)
}

fn stored(what: String) -> PortError {
    PortError::new("stored firewall rules", what)
}

/// A rule's `service`, `project`, `protocol` and `ports` columns.
fn service_columns(
    service: &Service,
) -> (&'static str, Option<String>, Option<&'static str>, Option<String>) {
    match service {
        Service::All => ("all", None, None, None),
        Service::Known(known) => (known.key(), None, None, None),
        Service::Project(name) => ("project", Some(name.clone()), None, None),
        Service::Ports { protocol, ports } => {
            ("ports", None, Some(protocol.as_str()), Some(ports_text(ports)))
        }
    }
}

fn service(
    kind: &str,
    project: Option<String>,
    protocol: Option<String>,
    ports: Option<String>,
) -> Result<Service, PortError> {
    match (kind, project, protocol, ports) {
        ("all", ..) => Ok(Service::All),
        ("project", Some(name), ..) => Ok(Service::Project(name)),
        ("ports", _, Some(protocol), Some(ports)) => Ok(Service::Ports {
            protocol: Protocol::parse(&protocol).ok_or_else(|| stored(format!("protocol {protocol}")))?,
            ports: parse_ports(&ports).map_err(|e| stored(e.0))?,
        }),
        (key, ..) => {
            KnownService::parse(key).map(Service::Known).ok_or_else(|| stored(format!("service {key}")))
        }
    }
}

fn action(text: &str) -> Result<Action, PortError> {
    Action::parse(text).ok_or_else(|| stored(format!("action {text}")))
}

#[async_trait]
impl FirewallRepository for SqliteFirewall {
    async fn settings(&self) -> Result<FirewallSettings, PortError> {
        let settings = sqlx::query!(
            r#"SELECT enabled AS "enabled: bool", fallback FROM firewall_settings WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(settings) = settings else { return Ok(FirewallSettings::default()) };
        let rows = sqlx::query!(
            r#"SELECT enabled AS "enabled: bool", interface, service, project, protocol, ports, source, action
               FROM firewall_rules ORDER BY position"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let rules = rows
            .into_iter()
            .map(|row| {
                Ok(Rule {
                    enabled: row.enabled,
                    interface: row.interface,
                    service: service(&row.service, row.project, row.protocol, row.ports)?,
                    source: Source::parse(&row.source).map_err(|e| stored(e.0))?,
                    action: action(&row.action)?,
                })
            })
            .collect::<Result<Vec<_>, PortError>>()?;
        Ok(FirewallSettings { enabled: settings.enabled, fallback: action(&settings.fallback)?, rules })
    }

    async fn save(&self, settings: &FirewallSettings) -> Result<(), PortError> {
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        let fallback = settings.fallback.as_str();
        sqlx::query!(
            "INSERT INTO firewall_settings (id, enabled, fallback) VALUES (1, ?, ?)
             ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled, fallback = excluded.fallback",
            settings.enabled,
            fallback
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        sqlx::query!("DELETE FROM firewall_rules").execute(&mut *tx).await.map_err(db_error)?;
        for (position, rule) in settings.rules.iter().enumerate() {
            let position = i64::try_from(position).map_err(|_| stored("too many rules".into()))?;
            let (kind, project, protocol, ports) = service_columns(&rule.service);
            let (source, action) = (rule.source.to_string(), rule.action.as_str());
            sqlx::query!(
                "INSERT INTO firewall_rules
                     (position, enabled, interface, service, project, protocol, ports, source, action)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                position,
                rule.enabled,
                rule.interface,
                kind,
                project,
                protocol,
                ports,
                source,
                action
            )
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)
    }

    async fn told_blocked(&self) -> Result<BTreeSet<String>, PortError> {
        let keys = sqlx::query_scalar!("SELECT service FROM firewall_blocked")
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(keys.into_iter().collect())
    }

    async fn save_told_blocked(&self, keys: &BTreeSet<String>) -> Result<(), PortError> {
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM firewall_blocked").execute(&mut *tx).await.map_err(db_error)?;
        for key in keys {
            sqlx::query!("INSERT INTO firewall_blocked (service) VALUES (?)", key)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }
        tx.commit().await.map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_firewall::domain::KnownService;

    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn rule(service: Service, source: &str, action: Action) -> Rule {
        Rule { enabled: true, interface: None, service, source: Source::parse(source).unwrap(), action }
    }

    #[tokio::test]
    async fn settings_come_back_as_saved_in_their_order() {
        let (_dir, db) = temporary_database().await;
        let firewall = SqliteFirewall::new(db.clone());
        assert_eq!(firewall.settings().await.unwrap(), FirewallSettings::default());
        let settings = FirewallSettings {
            enabled: true,
            fallback: Action::Deny,
            rules: vec![
                Rule {
                    interface: Some("enp1s0".into()),
                    ..rule(Service::Known(KnownService::Smb), "192.168.1.0/24", Action::Allow)
                },
                Rule { enabled: false, ..rule(Service::All, "10.0.0.1-10.0.0.9", Action::Deny) },
                rule(Service::Project("nextcloud".into()), "fd00::/8", Action::Allow),
                rule(
                    Service::Ports { protocol: Protocol::Udp, ports: parse_ports("53, 8000-8100").unwrap() },
                    "any",
                    Action::Allow,
                ),
                rule(Service::Known(KnownService::Ping), "10.0.0.1", Action::Allow),
            ],
        };
        firewall.save(&settings).await.unwrap();
        assert_eq!(firewall.settings().await.unwrap(), settings);
        let fewer = FirewallSettings { enabled: false, rules: settings.rules[2..].to_vec(), ..settings };
        firewall.save(&fewer).await.unwrap();
        assert_eq!(firewall.settings().await.unwrap(), fewer);

        sqlx::query("UPDATE firewall_rules SET source = 'nowhere' WHERE position = 0")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(firewall.settings().await.is_err(), "a broken row is said, not dropped");
        db.close().await;
    }

    #[tokio::test]
    async fn the_services_told_blocked_are_kept() {
        let (_dir, db) = temporary_database().await;
        let firewall = SqliteFirewall::new(db.clone());
        assert!(firewall.told_blocked().await.unwrap().is_empty());
        let keys: BTreeSet<String> = ["smb".to_owned(), "project:media".to_owned()].into();
        firewall.save_told_blocked(&keys).await.unwrap();
        assert_eq!(firewall.told_blocked().await.unwrap(), keys);
        firewall.save_told_blocked(&BTreeSet::new()).await.unwrap();
        assert!(firewall.told_blocked().await.unwrap().is_empty());
        db.close().await;
    }

    #[test]
    fn rows_that_make_no_sense_are_said() {
        assert!(service("smb", None, None, None).is_ok());
        assert!(service("telnet", None, None, None).is_err());
        assert!(service("ports", None, Some("icmp".into()), Some("1".into())).is_err());
        assert!(service("ports", None, Some("tcp".into()), Some("0".into())).is_err());
        assert!(action("reject").is_err());
    }
}
