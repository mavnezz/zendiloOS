//! How an account wants the interface to look. Kept with the account, so
//! that every browser it signs in with looks the same (docs/architecture.md,
//! 4.2).

use zendiloos_kernel::Timestamp;

/// Languages of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    German,
}

impl Language {
    /// Language tag, as stored and as the interface uses it.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::German => "de",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "en" => Language::English,
            "de" => Language::German,
            _ => return None,
        })
    }
}

/// Light or dark, or whatever the viewer's operating system uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
    System,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            "system" => Theme::System,
            _ => return None,
        })
    }
}

/// The name of a desktop background the interface offers: lowercase
/// letters, at most 32. Which ones exist is the interface's business; one
/// it does not know shows its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper(String);

impl Wallpaper {
    pub fn parse(value: &str) -> Option<Self> {
        let valid = (1..=32).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_lowercase());
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The colour of a desktop background of one colour (the wallpaper the
/// interface names `color`): `#rrggbb`, lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperColor(String);

impl WallpaperColor {
    pub fn parse(value: &str) -> Option<Self> {
        let valid = value.len() == 7
            && value.starts_with('#')
            && value.bytes().skip(1).all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        valid.then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What an administrator answered the reminder to star zendiloOS on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarAnswer {
    /// Once more, some time later.
    Later,
    /// Went to GitHub. Whether a star was given is nobody's business.
    Starred,
    Declined,
}

impl StarAnswer {
    pub fn as_str(self) -> &'static str {
        match self {
            StarAnswer::Later => "later",
            StarAnswer::Starred => "starred",
            StarAnswer::Declined => "declined",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "later" => StarAnswer::Later,
            "starred" => StarAnswer::Starred,
            "declined" => StarAnswer::Declined,
            _ => return None,
        })
    }
}

/// The reminder to star zendiloOS on GitHub, once it was shown: how often
/// (at most [`StarPrompt::MAX_SHOWN`] times), when last, and the answer.
/// When it is due is the interface's business; nothing of it leaves the
/// machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StarPrompt {
    times_shown: u8,
    last_shown: Timestamp,
    answer: Option<StarAnswer>,
}

impl StarPrompt {
    pub const MAX_SHOWN: u8 = 2;

    pub fn new(times_shown: u8, last_shown: Timestamp, answer: Option<StarAnswer>) -> Option<Self> {
        let valid = (1..=Self::MAX_SHOWN).contains(&times_shown) && last_shown.unix_seconds() >= 0;
        valid.then_some(Self { times_shown, last_shown, answer })
    }

    pub fn times_shown(self) -> u8 {
        self.times_shown
    }

    pub fn last_shown(self) -> Timestamp {
        self.last_shown
    }

    pub fn answer(self) -> Option<StarAnswer> {
        self.answer
    }

    /// Of what is kept and what a browser sends, the one further along:
    /// shown more often, later, or answered. A browser that loaded the
    /// preferences before the reminder appeared in another one thus does
    /// not bring it back by saving a theme.
    pub fn further(kept: Option<Self>, sent: Option<Self>) -> Option<Self> {
        let progress = |prompt: &Self| (prompt.times_shown, prompt.last_shown, prompt.answer.is_some());
        match (kept, sent) {
            (Some(kept), Some(sent)) if progress(&kept) > progress(&sent) => Some(kept),
            (kept, None) => kept,
            (_, sent) => sent,
        }
    }
}

/// An order of things on the desktop, by their ids: widgets, app icons. At
/// most 32, each a lowercase letter, then letters, digits and `-`, at most
/// 32 long, none twice. Ids the interface does not know are skipped there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout(Vec<String>);

impl Layout {
    pub const MAX_ITEMS: usize = 32;

    pub fn new(ids: Vec<String>) -> Option<Self> {
        let valid = ids.len() <= Self::MAX_ITEMS
            && ids.iter().enumerate().all(|(i, id)| {
                (1..=32).contains(&id.len())
                    && id.as_bytes()[0].is_ascii_lowercase()
                    && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    && !ids[..i].contains(id)
            });
        valid.then_some(Self(ids))
    }

    /// As stored: the ids joined by commas.
    pub fn parse(stored: &str) -> Option<Self> {
        let ids = if stored.is_empty() { Vec::new() } else { stored.split(',').map(str::to_owned).collect() };
        Self::new(ids)
    }

    pub fn ids(&self) -> &[String] {
        &self.0
    }

    pub fn joined(&self) -> String {
        self.0.join(",")
    }
}

/// An account's choices. `None` means not chosen yet: each browser keeps
/// what it shows, so nothing changes for someone who chose before the
/// choices were kept with the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    pub language: Option<Language>,
    pub theme: Option<Theme>,
    pub wallpaper: Option<Wallpaper>,
    /// The colour of the background of one colour, also while another one
    /// is chosen.
    pub wallpaper_color: Option<WallpaperColor>,
    /// The widgets beside the desktop, in order; empty shows none.
    pub widgets: Option<Layout>,
    /// The app icons on the desktop, in order.
    pub desktop_icons: Option<Layout>,
    /// `None` until the reminder to star zendiloOS was first shown.
    pub star_prompt: Option<StarPrompt>,
}

impl Preferences {
    /// These choices with those of `change` made: a choice it leaves out
    /// (`None`) stays as it is, and the reminder to star zendiloOS never
    /// goes back ([`StarPrompt::further`]). A browser thus changes only what
    /// was chosen in it, and one that knows less than is kept (its first
    /// load failed, or another tab chose since) takes nothing away.
    pub fn changed_by(self, change: Preferences) -> Self {
        Self {
            language: change.language.or(self.language),
            theme: change.theme.or(self.theme),
            wallpaper: change.wallpaper.or(self.wallpaper),
            wallpaper_color: change.wallpaper_color.or(self.wallpaper_color),
            widgets: change.widgets.or(self.widgets),
            desktop_icons: change.desktop_icons.or(self.desktop_icons),
            star_prompt: StarPrompt::further(self.star_prompt, change.star_prompt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_read_back() {
        for language in [Language::English, Language::German] {
            assert_eq!(Language::parse(language.code()), Some(language));
        }
        for theme in [Theme::Light, Theme::Dark, Theme::System] {
            assert_eq!(Theme::parse(theme.as_str()), Some(theme));
        }
        assert_eq!(Language::parse("fr"), None);
        assert_eq!(Theme::parse("Dark"), None);
    }

    #[test]
    fn wallpapers_and_layouts_are_plain_ids() {
        assert_eq!(Wallpaper::parse("aurora").unwrap().as_str(), "aurora");
        assert!(Wallpaper::parse("../x").is_none());
        assert!(Wallpaper::parse("").is_none());
        let layout = Layout::parse("status,resources,storage").unwrap();
        assert_eq!(layout.ids(), ["status", "resources", "storage"]);
        assert_eq!(Layout::parse(&layout.joined()), Some(layout));
        assert_eq!(Layout::parse("").unwrap().ids(), [] as [String; 0]);
        assert!(Layout::parse("status,status").is_none(), "twice");
        assert!(Layout::parse("Status").is_none());
        assert!(Layout::new(vec!["a".into(); 33]).is_none());
    }

    #[test]
    fn wallpaper_colours_are_lowercase_hex() {
        assert_eq!(WallpaperColor::parse("#1e293b").unwrap().as_str(), "#1e293b");
        for bad in ["#1E293B", "1e293b", "#1e293", "#1e293b0", "#1e29gb", "", "red", "#é9293b"] {
            assert!(WallpaperColor::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn star_answers_read_back() {
        for answer in [StarAnswer::Later, StarAnswer::Starred, StarAnswer::Declined] {
            assert_eq!(StarAnswer::parse(answer.as_str()), Some(answer));
        }
        assert_eq!(StarAnswer::parse("maybe"), None);
    }

    #[test]
    fn the_star_prompt_is_shown_once_or_twice() {
        let at = Timestamp::from_unix_seconds(1_700_000_000);
        let prompt = StarPrompt::new(1, at, Some(StarAnswer::Later)).unwrap();
        assert_eq!(
            (prompt.times_shown(), prompt.last_shown(), prompt.answer()),
            (1, at, Some(StarAnswer::Later))
        );
        assert!(StarPrompt::new(2, at, None).is_some());
        assert!(StarPrompt::new(0, at, None).is_none(), "never shown is no prompt");
        assert!(StarPrompt::new(3, at, None).is_none());
        assert!(StarPrompt::new(1, Timestamp::from_unix_seconds(-1), None).is_none());
    }

    #[test]
    fn the_star_prompt_never_goes_back() {
        let at = |seconds| Timestamp::from_unix_seconds(seconds);
        let shown = StarPrompt::new(1, at(100), None);
        let later = StarPrompt::new(1, at(100), Some(StarAnswer::Later));
        let declined = StarPrompt::new(1, at(100), Some(StarAnswer::Declined));
        let again = StarPrompt::new(2, at(200), Some(StarAnswer::Later));

        assert_eq!(StarPrompt::further(None, shown), shown);
        assert_eq!(StarPrompt::further(later, None), later, "a browser that knew nothing yet");
        assert_eq!(StarPrompt::further(declined, shown), declined, "one that missed the answer");
        assert_eq!(StarPrompt::further(again, later), again);
        assert_eq!(StarPrompt::further(later, again), again);
        assert_eq!(StarPrompt::further(shown, later), later);
        assert_eq!(StarPrompt::further(later, declined), declined, "answered in the same showing");
        assert_eq!(StarPrompt::further(None, None), None);
    }

    #[test]
    fn a_change_leaves_the_choices_it_does_not_make() {
        let kept = Preferences {
            language: Some(Language::German),
            theme: Some(Theme::Light),
            wallpaper: Wallpaper::parse("color"),
            wallpaper_color: WallpaperColor::parse("#134e4a"),
            widgets: Layout::parse("status"),
            desktop_icons: Layout::parse("files,storage"),
            star_prompt: StarPrompt::new(1, Timestamp::from_unix_seconds(100), Some(StarAnswer::Later)),
        };
        let theme = Preferences { theme: Some(Theme::Dark), ..Preferences::default() };
        assert_eq!(kept.clone().changed_by(theme), Preferences { theme: Some(Theme::Dark), ..kept.clone() });
        assert_eq!(kept.clone().changed_by(Preferences::default()), kept, "nothing sent, nothing changed");

        let everything = Preferences {
            language: Some(Language::English),
            theme: Some(Theme::System),
            wallpaper: Wallpaper::parse("tide"),
            wallpaper_color: WallpaperColor::parse("#dfe2e6"),
            widgets: Layout::parse(""),
            desktop_icons: Layout::parse("apps"),
            star_prompt: StarPrompt::new(2, Timestamp::from_unix_seconds(200), None),
        };
        assert_eq!(kept.changed_by(everything.clone()), everything);
    }
}
