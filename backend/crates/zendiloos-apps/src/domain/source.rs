//! Where apps come from: catalogs in the community catalog's format, each
//! published as one `index.json`.

use zendiloos_kernel::Timestamp;

/// The community catalog, built in.
pub const OFFICIAL_INDEX: &str = "https://mavnezz.github.io/zendiloOS.community.applications/v1/index.json";

/// The outcome of the last fetch of a source's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refresh {
    pub at: Timestamp,
    /// What went wrong; none if the index was read.
    pub error: Option<String>,
    /// Apps left out because they fail the format or the policy.
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: i64,
    /// Where its `index.json` lies.
    pub index: String,
    /// The community catalog: built in, cannot be removed.
    pub official: bool,
    pub added_at: Timestamp,
    pub refreshed: Option<Refresh>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSource {
    #[error("give a GitHub repository (github.com/owner/repository) or the https address of an index.json")]
    Address,
}

fn is_name(text: &str, extra: &[u8]) -> bool {
    (1..=100).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || extra.contains(&b))
        && !text.starts_with(['.', '-'])
}

/// The index address for what an administrator typed: a GitHub repository
/// (`github.com/owner/repo`, with or without `https://`), read as its
/// GitHub Pages site `https://owner.github.io/repo/v1/index.json`, or the
/// https address of an index itself.
pub fn index_address(input: &str) -> Result<String, InvalidSource> {
    let input = input.trim().trim_end_matches('/');
    let without_scheme = input.strip_prefix("https://").unwrap_or(input);
    if let Some(path) =
        without_scheme.strip_prefix("github.com/").or_else(|| without_scheme.strip_prefix("www.github.com/"))
    {
        let path = path.strip_suffix(".git").unwrap_or(path);
        let Some((owner, repository)) = path.split_once('/') else { return Err(InvalidSource::Address) };
        if !is_name(owner, b"") || !is_name(repository, b"._") {
            return Err(InvalidSource::Address);
        }
        return Ok(format!("https://{}.github.io/{repository}/v1/index.json", owner.to_ascii_lowercase()));
    }
    let Some(rest) = input.strip_prefix("https://") else { return Err(InvalidSource::Address) };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let bad = |c: char| c.is_whitespace() || c.is_control() || c == '\\';
    if host.is_empty() || host.contains('@') || input.len() > 2048 || input.chars().any(bad) {
        return Err(InvalidSource::Address);
    }
    if !rest.contains('/') {
        return Err(InvalidSource::Address);
    }
    Ok(input.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_become_their_pages_index() {
        let pages = "https://mavnezz.github.io/zendiloOS.community.applications/v1/index.json";
        for typed in [
            "github.com/mavnezz/zendiloOS.community.applications",
            "https://github.com/mavnezz/zendiloOS.community.applications/",
            " https://github.com/Mavnezz/zendiloOS.community.applications.git ",
        ] {
            assert_eq!(index_address(typed).unwrap(), pages, "{typed}");
        }
        assert_eq!(OFFICIAL_INDEX, pages);
    }

    #[test]
    fn index_addresses_are_https_without_tricks() {
        let own = "https://apps.example.org/catalog/v1/index.json";
        assert_eq!(index_address(own).unwrap(), own);
        for bad in [
            "http://apps.example.org/index.json",
            "https://apps.example.org",
            "https://user@apps.example.org/index.json",
            "https://apps.example.org/a b.json",
            "github.com/only-owner",
            "github.com/../etc",
            "ftp://x/y",
            "",
        ] {
            assert_eq!(index_address(bad), Err(InvalidSource::Address), "{bad:?}");
        }
    }
}
