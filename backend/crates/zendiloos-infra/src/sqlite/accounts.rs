use async_trait::async_trait;
use zendiloos_identity::application::ports::AccountRepository;
use zendiloos_identity::domain::{
    Account, AccountId, AccountName, NewAccount, PasswordHash, PasswordPolicy, Role,
};
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
    description: Option<String>,
    email: Option<String>,
    role: String,
    password_hash: String,
    password_change_required: bool,
    password_locked: bool,
    password_changed_at: i64,
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
            description: row.description,
            email: row.email,
            role,
            password_hash: PasswordHash::new(row.password_hash),
            password_change_required: row.password_change_required,
            password_locked: row.password_locked,
            password_changed_at: Timestamp::from_unix_seconds(row.password_changed_at),
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
            r#"SELECT id, name, full_name, description, email, role, password_hash,
                      password_change_required AS "password_change_required: bool",
                      password_locked AS "password_locked: bool", password_changed_at,
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
            r#"SELECT id, name, full_name, description, email, role, password_hash,
                      password_change_required AS "password_change_required: bool",
                      password_locked AS "password_locked: bool", password_changed_at,
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
                   (name, full_name, description, email, role, password_hash, password_change_required,
                    password_locked, password_changed_at, created_at, updated_at)
               SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9
               WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE role = 'admin')
               RETURNING id AS "id!""#,
            name,
            account.full_name,
            account.description,
            account.email,
            role,
            hash,
            account.password_change_required,
            account.password_locked,
            created_at
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;

        Ok(id.map(|id| account.stored(AccountId::new(id))))
    }

    async fn update_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError> {
        let (id, hash, at) = (id.get(), hash.as_str(), at.unix_seconds());
        sqlx::query!(
            "UPDATE accounts SET password_hash = ?1, password_change_required = 0, password_changed_at = ?2,
                 updated_at = ?2 WHERE id = ?3",
            hash,
            at,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Account>, PortError> {
        sqlx::query_as!(
            AccountRow,
            r#"SELECT id, name, full_name, description, email, role, password_hash,
                      password_change_required AS "password_change_required: bool",
                      password_locked AS "password_locked: bool", password_changed_at,
                      disabled AS "disabled: bool", created_at, updated_at
               FROM accounts ORDER BY name"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?
        .into_iter()
        .map(Account::try_from)
        .collect()
    }

    async fn create(&self, account: NewAccount) -> Result<Option<Account>, PortError> {
        let (name, role, hash) =
            (account.name.as_str(), account.role.as_str(), account.password_hash.as_str());
        let created_at = account.created_at.unix_seconds();
        let id = sqlx::query_scalar!(
            r#"INSERT INTO accounts
                   (name, full_name, description, email, role, password_hash, password_change_required,
                    password_locked, password_changed_at, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9)
               ON CONFLICT (name) DO NOTHING
               RETURNING id AS "id!""#,
            name,
            account.full_name,
            account.description,
            account.email,
            role,
            hash,
            account.password_change_required,
            account.password_locked,
            created_at
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;

        Ok(id.map(|id| account.stored(AccountId::new(id))))
    }

    async fn update(&self, account: &Account) -> Result<(), PortError> {
        let (id, role, at) = (account.id.get(), account.role.as_str(), account.updated_at.unix_seconds());
        sqlx::query!(
            "UPDATE accounts SET full_name = ?, description = ?, email = ?, role = ?, disabled = ?,
                 password_locked = ?, password_change_required = ?, updated_at = ? WHERE id = ?",
            account.full_name,
            account.description,
            account.email,
            role,
            account.disabled,
            account.password_locked,
            account.password_change_required,
            at,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn reset_password(
        &self,
        id: AccountId,
        hash: &PasswordHash,
        at: Timestamp,
    ) -> Result<(), PortError> {
        let (id, hash, at) = (id.get(), hash.as_str(), at.unix_seconds());
        sqlx::query!(
            "UPDATE accounts SET password_hash = ?1, password_change_required = NOT password_locked,
                 password_changed_at = ?2, updated_at = ?2 WHERE id = ?3",
            hash,
            at,
            id
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn delete(&self, id: AccountId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("DELETE FROM accounts WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn require_password_change(&self, id: AccountId) -> Result<(), PortError> {
        let id = id.get();
        sqlx::query!("UPDATE accounts SET password_change_required = 1 WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn password_policy(&self) -> Result<PasswordPolicy, PortError> {
        let row = sqlx::query!(
            r#"SELECT min_length, mixed_case AS "mixed_case: bool", digit AS "digit: bool",
                      symbol AS "symbol: bool", max_age_days FROM password_policy WHERE id = 1"#
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(PasswordPolicy::default()) };
        let stored = |what: &str| PortError::new("stored password policy", what.to_owned());
        Ok(PasswordPolicy {
            min_length: u8::try_from(row.min_length).map_err(|_| stored("minimum length"))?,
            mixed_case: row.mixed_case,
            digit: row.digit,
            symbol: row.symbol,
            max_age_days: row
                .max_age_days
                .map(u16::try_from)
                .transpose()
                .map_err(|_| stored("maximum age"))?,
        })
    }

    async fn set_password_policy(&self, policy: &PasswordPolicy) -> Result<(), PortError> {
        let (min_length, max_age_days) = (i64::from(policy.min_length), policy.max_age_days.map(i64::from));
        sqlx::query!(
            "INSERT INTO password_policy (id, min_length, mixed_case, digit, symbol, max_age_days)
             VALUES (1, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET min_length = excluded.min_length, mixed_case = excluded.mixed_case,
                 digit = excluded.digit, symbol = excluded.symbol, max_age_days = excluded.max_age_days",
            min_length,
            policy.mixed_case,
            policy.digit,
            policy.symbol,
            max_age_days
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
            description: None,
            email: None,
            role,
            password_hash: PasswordHash::new("$argon2id$hash"),
            password_change_required: true,
            password_locked: false,
            created_at: Timestamp::from_unix_seconds(1000),
        }
    }

    #[tokio::test]
    async fn keeps_the_password_policy_and_locked_passwords() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db);
        assert_eq!(accounts.password_policy().await.unwrap(), PasswordPolicy::default());
        let policy = PasswordPolicy {
            min_length: 14,
            symbol: true,
            max_age_days: Some(90),
            ..PasswordPolicy::default()
        };
        accounts.set_password_policy(&policy).await.unwrap();
        accounts.set_password_policy(&policy).await.unwrap();
        assert_eq!(accounts.password_policy().await.unwrap(), policy);

        let mut locked = new_account("kiosk", Role::User);
        locked.password_locked = true;
        locked.description = Some("Kitchen tablet".into());
        let kiosk = accounts.create(locked).await.unwrap().unwrap();
        let at = Timestamp::from_unix_seconds(5_000);
        accounts.reset_password(kiosk.id, &PasswordHash::new("$argon2id$new"), at).await.unwrap();
        let stored = accounts.find_by_id(kiosk.id).await.unwrap().unwrap();
        assert!(!stored.password_change_required, "only administrators set it anyway");
        assert_eq!((stored.password_changed_at, stored.description.as_deref()), (at, Some("Kitchen tablet")));
        accounts.require_password_change(kiosk.id).await.unwrap();
        assert!(accounts.find_by_id(kiosk.id).await.unwrap().unwrap().password_change_required);
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

    #[tokio::test]
    async fn creates_lists_updates_and_deletes_accounts() {
        let (_dir, db) = temporary_database().await;
        let accounts = SqliteAccounts::new(db.clone());
        let mut carol = accounts.create(new_account("carol", Role::User)).await.unwrap().unwrap();
        let bob = accounts.create(new_account("bob", Role::ReadOnly)).await.unwrap().unwrap();
        assert_eq!(accounts.create(new_account("bob", Role::Admin)).await.unwrap(), None, "name taken");

        let names: Vec<_> = accounts.list().await.unwrap().into_iter().map(|a| a.name.to_string()).collect();
        assert_eq!(names, ["bob", "carol"]);

        carol.full_name = None;
        carol.role = Role::Admin;
        carol.disabled = true;
        carol.updated_at = Timestamp::from_unix_seconds(3000);
        accounts.update(&carol).await.unwrap();
        assert_eq!(accounts.find_by_id(carol.id).await.unwrap(), Some(carol.clone()));

        let hash = PasswordHash::new("$argon2id$reset");
        accounts.reset_password(bob.id, &hash, Timestamp::from_unix_seconds(4000)).await.unwrap();
        let stored = accounts.find_by_id(bob.id).await.unwrap().unwrap();
        assert!(stored.password_change_required);
        assert_eq!(stored.password_hash, hash);

        accounts.delete(bob.id).await.unwrap();
        assert_eq!(accounts.find_by_id(bob.id).await.unwrap(), None);
        db.close().await;
    }
}
