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

/// An account's choices. `None` means not chosen yet: each browser keeps
/// what it shows, so nothing changes for someone who chose before the
/// choices were kept with the account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Preferences {
    pub language: Option<Language>,
    pub theme: Option<Theme>,
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
}
