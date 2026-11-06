//! How an account wants the interface to look. Kept with the account, so
//! that every browser it signs in with looks the same (docs/architecture.md,
//! 4.2).

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
    /// The widgets beside the desktop, in order; empty shows none.
    pub widgets: Option<Layout>,
    /// The app icons on the desktop, in order.
    pub desktop_icons: Option<Layout>,
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
}
