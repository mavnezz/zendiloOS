use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_sharing::application::ports::{Directory, Names, ShareRepository};
use zendiloos_sharing::domain::{
    Access, AccessRule, NewShare, NfsClient, Share, ShareId, ShareName, Subject,
};

use super::Database;

/// [`ShareRepository`] on the `shares` tables, and the [`Directory`] of
/// account and group names.
#[derive(Debug, Clone)]
pub struct SqliteShares {
    db: Database,
}

impl SqliteShares {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("shares", error)
}

fn stored(what: impl Into<String>) -> PortError {
    PortError::new("stored share", what.into())
}

struct ShareRow {
    id: i64,
    name: String,
    description: Option<String>,
    pool: String,
    volume: i64,
    recycle_bin: bool,
    created_at: i64,
    updated_at: i64,
}

async fn replace_rules(
    tx: &mut sqlx::SqliteConnection,
    id: i64,
    access: &[AccessRule],
    nfs: &[NfsClient],
) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM share_access WHERE share_id = ?", id).execute(&mut *tx).await?;
    sqlx::query!("DELETE FROM share_nfs_clients WHERE share_id = ?", id).execute(&mut *tx).await?;
    for (position, rule) in access.iter().enumerate() {
        let (account, group) = match rule.subject {
            Subject::Account(account) => (Some(account), None),
            Subject::Group(group) => (None, Some(group)),
        };
        let level = match rule.access {
            Access::Read => "read",
            Access::ReadWrite => "read_write",
        };
        let position = position as i64;
        sqlx::query!(
            "INSERT INTO share_access (share_id, account_id, group_id, access, position) VALUES (?, ?, ?, ?, ?)",
            id,
            account,
            group,
            level,
            position
        )
        .execute(&mut *tx)
        .await?;
    }
    for (position, client) in nfs.iter().enumerate() {
        let position = position as i64;
        sqlx::query!(
            "INSERT INTO share_nfs_clients (share_id, network, write, position) VALUES (?, ?, ?, ?)",
            id,
            client.network,
            client.write,
            position
        )
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

impl SqliteShares {
    async fn load(&self, rows: Vec<ShareRow>) -> Result<Vec<Share>, PortError> {
        let rules = sqlx::query!(
            r#"SELECT share_id AS "share_id!", account_id, group_id, access FROM share_access ORDER BY position"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let clients = sqlx::query!(
            r#"SELECT share_id AS "share_id!", network AS "network!", write AS "write!: bool"
               FROM share_nfs_clients ORDER BY position"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let mut access: HashMap<i64, Vec<AccessRule>> = HashMap::new();
        for rule in rules {
            let subject = match (rule.account_id, rule.group_id) {
                (Some(account), None) => Subject::Account(account),
                (None, Some(group)) => Subject::Group(group),
                _ => return Err(stored("a rule without exactly one subject")),
            };
            let level = match rule.access.as_str() {
                "read" => Access::Read,
                "read_write" => Access::ReadWrite,
                other => return Err(stored(format!("access {other:?}"))),
            };
            access.entry(rule.share_id).or_default().push(AccessRule { subject, access: level });
        }
        let mut nfs: HashMap<i64, Vec<NfsClient>> = HashMap::new();
        for client in clients {
            nfs.entry(client.share_id)
                .or_default()
                .push(NfsClient { network: client.network, write: client.write });
        }
        rows.into_iter()
            .map(|row| {
                Ok(Share {
                    id: ShareId::new(row.id),
                    name: ShareName::parse(&row.name).map_err(|e| stored(e.to_string()))?,
                    description: row.description,
                    pool: row.pool,
                    volume: u16::try_from(row.volume).map_err(|_| stored("volume"))?,
                    recycle_bin: row.recycle_bin,
                    access: access.remove(&row.id).unwrap_or_default(),
                    nfs: nfs.remove(&row.id).unwrap_or_default(),
                    created_at: Timestamp::from_unix_seconds(row.created_at),
                    updated_at: Timestamp::from_unix_seconds(row.updated_at),
                })
            })
            .collect()
    }
}

#[async_trait]
impl ShareRepository for SqliteShares {
    async fn list(&self) -> Result<Vec<Share>, PortError> {
        let rows = sqlx::query_as!(
            ShareRow,
            r#"SELECT id AS "id!", name, description, pool, volume, recycle_bin AS "recycle_bin: bool",
                      created_at, updated_at
               FROM shares ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        self.load(rows).await
    }

    async fn find(&self, id: ShareId) -> Result<Option<Share>, PortError> {
        Ok(self.list().await?.into_iter().find(|share| share.id == id))
    }

    async fn create(&self, new: NewShare) -> Result<Option<Share>, PortError> {
        let (name, at, volume) = (new.name.as_str(), new.created_at.unix_seconds(), i64::from(new.volume));
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        let id = sqlx::query_scalar!(
            r#"INSERT INTO shares (name, description, pool, volume, recycle_bin, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
               ON CONFLICT (name) DO NOTHING
               RETURNING id AS "id!""#,
            name,
            new.description,
            new.pool,
            volume,
            new.recycle_bin,
            at
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(id) = id else { return Ok(None) };
        replace_rules(&mut tx, id, &new.access, &new.nfs).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(Share {
            id: ShareId::new(id),
            name: new.name,
            description: new.description,
            pool: new.pool,
            volume: new.volume,
            recycle_bin: new.recycle_bin,
            access: new.access,
            nfs: new.nfs,
            created_at: new.created_at,
            updated_at: new.created_at,
        }))
    }

    async fn update(&self, share: &Share) -> Result<(), PortError> {
        let (id, at) = (share.id.get(), share.updated_at.unix_seconds());
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!(
            "UPDATE shares SET description = ?, recycle_bin = ?, updated_at = ? WHERE id = ?",
            share.description,
            share.recycle_bin,
            at,
            id
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        replace_rules(&mut tx, id, &share.access, &share.nfs).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    async fn delete(&self, id: ShareId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("DELETE FROM shares WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }
}

#[async_trait]
impl Directory for SqliteShares {
    async fn names(&self) -> Result<Names, PortError> {
        let accounts = sqlx::query!(r#"SELECT id AS "id!", name FROM accounts"#)
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        let groups = sqlx::query!(r#"SELECT id AS "id!", name FROM groups"#)
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(Names {
            accounts: accounts.into_iter().map(|a| (a.id, a.name)).collect(),
            groups: groups.into_iter().map(|g| (g.id, g.name)).collect(),
        })
    }

    async fn memberships(&self, account: i64) -> Result<Vec<i64>, PortError> {
        sqlx::query_scalar!(
            r#"SELECT group_id AS "group_id!" FROM group_members WHERE account_id = ?"#,
            account
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_identity::application::ports::{AccountRepository, GroupRepository};
    use zendiloos_identity::domain::{AccountName, GroupName, NewAccount, NewGroup, PasswordHash, Role};

    use super::*;
    use crate::sqlite::tests::temporary_database;
    use crate::sqlite::{SqliteAccounts, SqliteGroups};

    #[tokio::test]
    async fn stores_shares_with_rules_that_follow_accounts_and_groups() {
        let (_dir, db) = temporary_database().await;
        let at = Timestamp::from_unix_seconds(100);
        let alice = SqliteAccounts::new(db.clone())
            .create(NewAccount {
                name: AccountName::parse("alice").unwrap(),
                full_name: None,
                role: Role::User,
                password_hash: PasswordHash::new("$argon2id$x"),
                password_change_required: false,
                created_at: at,
            })
            .await
            .unwrap()
            .unwrap();
        let family = SqliteGroups::new(db.clone())
            .create(NewGroup {
                name: GroupName::parse("family").unwrap(),
                description: None,
                members: vec![alice.id],
                created_at: at,
            })
            .await
            .unwrap()
            .unwrap();
        let shares = SqliteShares::new(db.clone());
        let new = NewShare {
            name: ShareName::parse("photos").unwrap(),
            description: Some("Pictures".into()),
            pool: "tank".into(),
            volume: 1,
            recycle_bin: true,
            access: vec![
                AccessRule { subject: Subject::Group(family.id.get()), access: Access::ReadWrite },
                AccessRule { subject: Subject::Account(alice.id.get()), access: Access::Read },
            ],
            nfs: vec![NfsClient { network: "192.168.1.0/24".into(), write: false }],
            created_at: at,
        };
        let mut share = shares.create(new.clone()).await.unwrap().unwrap();
        assert_eq!(shares.create(new).await.unwrap(), None, "name taken");
        assert_eq!(shares.find(share.id).await.unwrap(), Some(share.clone()));
        let names = shares.names().await.unwrap();
        assert_eq!(names.accounts[&alice.id.get()], "alice");
        assert_eq!(names.groups[&family.id.get()], "family");
        assert_eq!(shares.memberships(alice.id.get()).await.unwrap(), [family.id.get()]);
        assert!(shares.memberships(alice.id.get() + 1).await.unwrap().is_empty());

        share.nfs.clear();
        share.recycle_bin = false;
        shares.update(&share).await.unwrap();
        assert_eq!(shares.list().await.unwrap(), [share.clone()]);

        SqliteAccounts::new(db.clone()).delete(alice.id).await.unwrap();
        let rules = shares.find(share.id).await.unwrap().unwrap().access;
        assert_eq!(
            rules,
            [AccessRule { subject: Subject::Group(family.id.get()), access: Access::ReadWrite }]
        );

        shares.delete(share.id).await.unwrap();
        assert!(shares.list().await.unwrap().is_empty());
        db.close().await;
    }
}
