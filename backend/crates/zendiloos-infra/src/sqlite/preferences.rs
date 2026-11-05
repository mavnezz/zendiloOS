use async_trait::async_trait;
use zendiloos_identity::application::ports::PreferenceRepository;
use zendiloos_identity::domain::{AccountId, Language, Preferences, Theme};
use zendiloos_kernel::PortError;

use super::Database;

/// [`PreferenceRepository`] on the `account_preferences` table.
#[derive(Debug, Clone)]
pub struct SqlitePreferences {
    db: Database,
}

impl SqlitePreferences {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("preferences", error)
}

#[async_trait]
impl PreferenceRepository for SqlitePreferences {
    async fn load(&self, account: AccountId) -> Result<Preferences, PortError> {
        let id = account.get();
        let Some(row) =
            sqlx::query!("SELECT language, theme FROM account_preferences WHERE account_id = ?", id)
                .fetch_optional(self.db.pool())
                .await
                .map_err(db_error)?
        else {
            return Ok(Preferences::default());
        };
        // A value this version does not know counts as not chosen.
        Ok(Preferences {
            language: row.language.as_deref().and_then(Language::parse),
            theme: row.theme.as_deref().and_then(Theme::parse),
        })
    }

    async fn save(&self, account: AccountId, preferences: Preferences) -> Result<(), PortError> {
        let id = account.get();
        let language = preferences.language.map(Language::code);
        let theme = preferences.theme.map(Theme::as_str);
        sqlx::query!(
            "INSERT INTO account_preferences (account_id, language, theme) VALUES (?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET language = excluded.language, theme = excluded.theme",
            id,
            language,
            theme
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
    use crate::sqlite::tests::{insert_account, temporary_database};

    #[tokio::test]
    async fn keeps_choices_per_account() {
        let (_dir, db) = temporary_database().await;
        let repository = SqlitePreferences::new(db.clone());
        let alice = AccountId::new(insert_account(db.pool(), "alice", "admin").await.unwrap());
        let bob = AccountId::new(insert_account(db.pool(), "bob", "user").await.unwrap());
        assert_eq!(repository.load(alice).await.unwrap(), Preferences::default());

        let chosen = Preferences { language: Some(Language::German), theme: Some(Theme::Dark) };
        repository.save(alice, chosen).await.unwrap();
        repository.save(bob, Preferences { language: None, theme: Some(Theme::Light) }).await.unwrap();
        let changed = Preferences { language: Some(Language::German), theme: None };
        repository.save(alice, changed).await.unwrap();

        assert_eq!(repository.load(alice).await.unwrap(), changed);
        assert_eq!(repository.load(bob).await.unwrap().theme, Some(Theme::Light));
        db.close().await;
    }

    #[tokio::test]
    async fn the_database_refuses_unknown_themes() {
        let (_dir, db) = temporary_database().await;
        let alice = insert_account(db.pool(), "alice", "admin").await.unwrap();

        let stored = sqlx::query("INSERT INTO account_preferences (account_id, theme) VALUES (?, 'neon')")
            .bind(alice)
            .execute(db.pool())
            .await;

        assert!(stored.is_err());
        db.close().await;
    }
}
