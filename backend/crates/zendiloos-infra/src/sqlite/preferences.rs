use async_trait::async_trait;
use zendiloos_identity::application::ports::PreferenceRepository;
use zendiloos_identity::domain::{
    AccountId, Language, Layout, Preferences, StarAnswer, StarPrompt, Theme, Wallpaper, WallpaperColor,
};
use zendiloos_kernel::{PortError, Timestamp};

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
        let Some(row) = sqlx::query!(
            "SELECT language, theme, wallpaper, wallpaper_color, widgets, desktop_icons,
                star_prompt_shown, star_prompt_shown_at, star_prompt_answer
             FROM account_preferences WHERE account_id = ?",
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?
        else {
            return Ok(Preferences::default());
        };
        // A value this version does not know counts as not chosen.
        let star_prompt = row.star_prompt_shown_at.and_then(|at| {
            let answer = row.star_prompt_answer.as_deref().and_then(StarAnswer::parse);
            StarPrompt::new(
                u8::try_from(row.star_prompt_shown).ok()?,
                Timestamp::from_unix_seconds(at),
                answer,
            )
        });
        Ok(Preferences {
            language: row.language.as_deref().and_then(Language::parse),
            theme: row.theme.as_deref().and_then(Theme::parse),
            wallpaper: row.wallpaper.as_deref().and_then(Wallpaper::parse),
            wallpaper_color: row.wallpaper_color.as_deref().and_then(WallpaperColor::parse),
            widgets: row.widgets.as_deref().and_then(Layout::parse),
            desktop_icons: row.desktop_icons.as_deref().and_then(Layout::parse),
            star_prompt,
        })
    }

    async fn save(&self, account: AccountId, preferences: Preferences) -> Result<(), PortError> {
        let id = account.get();
        let language = preferences.language.map(Language::code);
        let theme = preferences.theme.map(Theme::as_str);
        let wallpaper = preferences.wallpaper.as_ref().map(Wallpaper::as_str);
        let color = preferences.wallpaper_color.as_ref().map(WallpaperColor::as_str);
        let widgets = preferences.widgets.as_ref().map(Layout::joined);
        let icons = preferences.desktop_icons.as_ref().map(Layout::joined);
        let prompt = preferences.star_prompt;
        let shown = prompt.map_or(0, |prompt| i64::from(prompt.times_shown()));
        let shown_at = prompt.map(|prompt| prompt.last_shown().unix_seconds());
        let answer = prompt.and_then(StarPrompt::answer).map(StarAnswer::as_str);
        sqlx::query!(
            "INSERT INTO account_preferences (account_id, language, theme, wallpaper, wallpaper_color, widgets,
                desktop_icons, star_prompt_shown, star_prompt_shown_at, star_prompt_answer)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (account_id) DO UPDATE SET language = excluded.language, theme = excluded.theme,
                wallpaper = excluded.wallpaper, wallpaper_color = excluded.wallpaper_color,
                widgets = excluded.widgets, desktop_icons = excluded.desktop_icons,
                star_prompt_shown = excluded.star_prompt_shown, star_prompt_shown_at = excluded.star_prompt_shown_at,
                star_prompt_answer = excluded.star_prompt_answer",
            id,
            language,
            theme,
            wallpaper,
            color,
            widgets,
            icons,
            shown,
            shown_at,
            answer
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

        let chosen = Preferences {
            language: Some(Language::German),
            theme: Some(Theme::Dark),
            ..Preferences::default()
        };
        repository.save(alice, chosen).await.unwrap();
        let light = Preferences { theme: Some(Theme::Light), ..Preferences::default() };
        repository.save(bob, light).await.unwrap();
        let changed = Preferences {
            language: Some(Language::German),
            theme: None,
            wallpaper: Wallpaper::parse("color"),
            wallpaper_color: WallpaperColor::parse("#134e4a"),
            widgets: Layout::parse("status,storage"),
            desktop_icons: Layout::parse("files,storage,apps"),
            star_prompt: StarPrompt::new(
                2,
                Timestamp::from_unix_seconds(1_700_000_000),
                Some(StarAnswer::Later),
            ),
        };
        repository.save(alice, changed.clone()).await.unwrap();

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

    #[tokio::test]
    async fn the_database_refuses_odd_colours_and_a_third_star_prompt() {
        let (_dir, db) = temporary_database().await;
        let alice = insert_account(db.pool(), "alice", "admin").await.unwrap();

        for odd in [
            "INSERT INTO account_preferences (account_id, wallpaper_color) VALUES (?, '#ABCDEF')",
            "INSERT INTO account_preferences (account_id, wallpaper_color) VALUES (?, '#abc')",
            "INSERT INTO account_preferences (account_id, star_prompt_shown) VALUES (?, 3)",
        ] {
            let stored = sqlx::query(odd).bind(alice).execute(db.pool()).await;
            assert!(stored.is_err(), "{odd}");
        }
        db.close().await;
    }
}
