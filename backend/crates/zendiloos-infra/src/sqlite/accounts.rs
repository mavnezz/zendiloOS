use async_trait::async_trait;
use zendiloos_identity::application::ports::AccountRepository;
use zendiloos_identity::domain::{Account, AccountId, AccountName, NewAccount, PasswordHash, Role};
use zendiloos_kernel::{PortError, Timestamp};

use super::Database;

/// [`AccountRepository`] on the `accounts` table.
#[derive(Debug, Clone)]
pub struct SqliteAccounts {
    db: Database,
}

impl SqliteAccounts {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("accounts", error)
}

struct AccountRow {
    id: i64,
    name: String,
    full_name: Option<String>,
    role: String,
    password_hash: String,
    password_change_required: bool,
    disabled: bool,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<AccountRow> for Account {
    type Error = PortError;

    fn try_from(row: AccountRow) -> Result<Self, PortError> {
        let name = AccountName::parse(&row.name).map_err(|error| PortError::new("stored account", error))?;
        let role = Role::parse(&row.role)
            .ok_or_else(|| PortError::new("stored account", format!("unknown role {:?}", row.role)))?;
        Ok(Account {
            id: AccountId::new(row.id),
            name,
            full_name: row.full_name,
            role,
            password_hash: PasswordHash::new(row.password_hash),
            password_change_required: row.password_change_required,
            disabled: row.disabled,
            created_at: Timestamp::from_unix_seconds(row.created_at),
            updated_at: Timestamp::from_unix_seconds(row.updated_at),
        })
    }
}

#[async_trait]
impl AccountRepository for SqliteAccounts {
    async fn any_admin(&self) -> Result<bool, PortError> {
        sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM accounts WHERE role = 'admin') AS "exists!: bool""#
        )
        .fetch_one(self.db.pool())
        .await
        .map_err(db_error)
    }

    async fn find_by_id(&self, id: AccountId) -> Result<Option<Account>, PortError> {
        let id = id.get();
        sqlx::query_as!(
            AccountRow,
            r#"SELECT id, name, full_name, role, password_hash,
                      password_change_required AS "password_change_required: bool",
                      disabled AS "disabled: bool", created_at, updated_at
               FROM accounts WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        .map(Account::try_from)
        .transpose()
    }

    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, PortError> {
        let name = name.as_str();
        sqlx::query_as!(
            AccountRow,
            r#"SELECT id, name, full_name, role, password_hash,
                      password_change_required AS "password_change_required: bool",
                      disabled AS "disabled: bool", created_at, updated_at
               FROM accounts WHERE name = ?"#,
            name
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        .map(Account::try_from)
        .transpose()
    }

    async fn create_first_admin(&self, account: NewAccount) -> Result<Option<Account>, PortError> {
        let (name, role, hash) =
            (account.name.as_str(), account.role.as_str(), account.password_hash.as_str());
        let created_at = account.created_at.unix_seconds();
        // One statement, so the check and the insert cannot be interleaved
        // with a concurrent setup.
        let id = sqlx::query_scalar!(
            r#"INSERT INTO accounts
                   (name, full_name, role, password_hash, password_change_required, created_at, updated_at)
               SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?6
               WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE role = 'admin')
               RETURNING id AS "id!""#,
            name,
            account.full_name,
            role,
            hash,
            account.password_change_required,
            created_at
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;

        Ok(id.map(|id| Account {
            id: AccountId::new(id),
            name: account.name,
            full_name: account.full_name,
            role: account.role,
            password_hash: account.password_hash,
            password_change_required: account.password_change_required,
            disabled: false,
            created_at: account.created_at,
            updated_at: account.created_at,
        }))
    }

    async fn update_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError> {
        let (id, hash, at) = (id.get(), hash.as_str(), at.unix_seconds());
        sqlx::query!(
            "UPDATE accounts SET password_hash = ?, password_change_required = 0, updated_at = ? WHERE id = ?",
            hash,
            at,
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

    fn new_account(name: &str, role: Role) -> NewAccount {
        NewAccount {
            name: AccountName::parse(name).unwrap(),
            full_name: Some("Full Name".to_owned()),
            role,
            password_hash: PasswordHash::new("$argon2id$hash"),
            password_change_required: true,
            created_at: Timestamp::from_unix_seconds(1000),
        }
    }

    #[tokio::test]
    async fn creates_only_the_first_admin() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db.clone());
        assert!(!accounts.any_admin().await.unwrap());

        let alice = accounts.create_first_admin(new_account("alice", Role::Admin)).await.unwrap().unwrap();
        let mallory = accounts.create_first_admin(new_account("mallory", Role::Admin)).await.unwrap();

        assert!(mallory.is_none());
        assert!(accounts.any_admin().await.unwrap());
        assert_eq!(accounts.find_by_id(alice.id).await.unwrap(), Some(alice.clone()));
        assert_eq!(accounts.find_by_name(&alice.name).await.unwrap(), Some(alice));
        assert_eq!(accounts.find_by_name(&AccountName::parse("mallory").unwrap()).await.unwrap(), None);
        db.close().await;
    }

    #[tokio::test]
    async fn updating_the_password_clears_the_change_requirement() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db.clone());
        let alice = accounts.create_first_admin(new_account("alice", Role::Admin)).await.unwrap().unwrap();

        let new_hash = PasswordHash::new("$argon2id$new");
        accounts.update_password(alice.id, &new_hash, Timestamp::from_unix_seconds(2000)).await.unwrap();

        let stored = accounts.find_by_id(alice.id).await.unwrap().unwrap();
        assert_eq!(stored.password_hash, new_hash);
        assert!(!stored.password_change_required);
        assert_eq!(stored.updated_at, Timestamp::from_unix_seconds(2000));
        db.close().await;
    }
}
