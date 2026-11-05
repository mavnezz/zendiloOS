use std::collections::HashMap;

use async_trait::async_trait;
use zendiloos_identity::application::ports::GroupRepository;
use zendiloos_identity::domain::{AccountId, Group, GroupId, GroupName, NewGroup};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`GroupRepository`] on the `groups` and `group_members` tables.
#[derive(Debug, Clone)]
pub struct SqliteGroups {
    db: Database,
}

impl SqliteGroups {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("groups", error)
}

struct GroupRow {
    id: i64,
    name: String,
    description: Option<String>,
    created_at: i64,
    updated_at: i64,
}

fn group(row: GroupRow, members: Vec<AccountId>) -> Result<Group, PortError> {
    let name = GroupName::parse(&row.name).map_err(|error| PortError::new("stored group", error))?;
    Ok(Group {
        id: GroupId::new(row.id),
        name,
        description: row.description,
        members,
        created_at: Timestamp::from_unix_seconds(row.created_at),
        updated_at: Timestamp::from_unix_seconds(row.updated_at),
    })
}

async fn replace_members(
    tx: &mut sqlx::SqliteConnection,
    group: i64,
    members: &[AccountId],
) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM group_members WHERE group_id = ?", group).execute(&mut *tx).await?;
    for member in members {
        let member = member.get();
        sqlx::query!("INSERT INTO group_members (group_id, account_id) VALUES (?, ?)", group, member)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

#[async_trait]
impl GroupRepository for SqliteGroups {
    async fn list(&self) -> Result<Vec<Group>, PortError> {
        let rows = sqlx::query_as!(
            GroupRow,
            r#"SELECT id AS "id!", name, description, created_at, updated_at FROM groups ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        let mut members: HashMap<i64, Vec<AccountId>> = HashMap::new();
        let pairs = sqlx::query!(
            r#"SELECT group_id AS "group_id!", account_id AS "account_id!" FROM group_members ORDER BY account_id"#
        )
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        for pair in pairs {
            members.entry(pair.group_id).or_default().push(AccountId::new(pair.account_id));
        }
        rows.into_iter()
            .map(|row| {
                let ids = members.remove(&row.id).unwrap_or_default();
                group(row, ids)
            })
            .collect()
    }

    async fn find_by_id(&self, id: GroupId) -> Result<Option<Group>, PortError> {
        let id = id.get();
        let Some(row) = sqlx::query_as!(
            GroupRow,
            r#"SELECT id AS "id!", name, description, created_at, updated_at FROM groups WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        else {
            return Ok(None);
        };
        let members = sqlx::query_scalar!(
            r#"SELECT account_id AS "account_id!" FROM group_members WHERE group_id = ? ORDER BY account_id"#,
            id
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        group(row, members.into_iter().map(AccountId::new).collect()).map(Some)
    }

    async fn create(&self, new: NewGroup) -> Result<Option<Group>, PortError> {
        let (name, at) = (new.name.as_str(), new.created_at.unix_seconds());
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        let id = sqlx::query_scalar!(
            r#"INSERT INTO groups (name, description, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
               ON CONFLICT (name) DO NOTHING
               RETURNING id AS "id!""#,
            name,
            new.description,
            at
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(id) = id else { return Ok(None) };
        replace_members(&mut tx, id, &new.members).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        Ok(Some(Group {
            id: GroupId::new(id),
            name: new.name,
            description: new.description,
            members: new.members,
            created_at: new.created_at,
            updated_at: new.created_at,
        }))
    }

    async fn update(&self, group: &Group) -> Result<(), PortError> {
        let (id, at) = (group.id.get(), group.updated_at.unix_seconds());
        let mut tx = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!(
            "UPDATE groups SET description = ?, updated_at = ? WHERE id = ?",
            group.description,
            at,
            id
        )
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        replace_members(&mut tx, id, &group.members).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    async fn delete(&self, id: GroupId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("DELETE FROM groups WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_identity::application::ports::AccountRepository;
    use zendiloos_identity::domain::{AccountName, NewAccount, PasswordHash, Role};

    use super::*;
    use crate::sqlite::SqliteAccounts;
    use crate::sqlite::tests::temporary_database;

    async fn account(accounts: &SqliteAccounts, name: &str) -> AccountId {
        let new = NewAccount {
            name: AccountName::parse(name).unwrap(),
            full_name: None,
            role: Role::User,
            password_hash: PasswordHash::new("$argon2id$hash"),
            password_change_required: false,
            created_at: Timestamp::from_unix_seconds(1000),
        };
        accounts.create(new).await.unwrap().unwrap().id
    }

    fn new_group(name: &str, members: Vec<AccountId>) -> NewGroup {
        NewGroup {
            name: GroupName::parse(name).unwrap(),
            description: Some("Shared photos".to_owned()),
            members,
            created_at: Timestamp::from_unix_seconds(2000),
        }
    }

    #[tokio::test]
    async fn stores_groups_with_their_members() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db.clone());
        let groups = SqliteGroups::new(db.clone());
        let (alice, bob) = (account(&accounts, "alice").await, account(&accounts, "bob").await);

        let mut family = groups.create(new_group("family", vec![alice, bob])).await.unwrap().unwrap();
        groups.create(new_group("admins", vec![])).await.unwrap().unwrap();
        assert_eq!(groups.create(new_group("family", vec![])).await.unwrap(), None, "name taken");
        assert_eq!(groups.find_by_id(family.id).await.unwrap(), Some(family.clone()));

        family.members = vec![bob];
        family.description = None;
        family.updated_at = Timestamp::from_unix_seconds(3000);
        groups.update(&family).await.unwrap();
        let listed = groups.list().await.unwrap();
        let names: Vec<_> = listed.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["admins", "family"]);
        assert_eq!(listed[1], family);

        // Deleting an account removes it from its groups.
        accounts.delete(bob).await.unwrap();
        assert_eq!(groups.find_by_id(family.id).await.unwrap().unwrap().members, []);

        groups.delete(family.id).await.unwrap();
        assert_eq!(groups.find_by_id(family.id).await.unwrap(), None);
        db.close().await;
    }
}
