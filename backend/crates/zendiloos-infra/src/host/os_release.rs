//! Minimal reader for os-release(5).

/// Returns the unquoted value of `PRETTY_NAME`, if present and not empty.
/// As in a shell, the last assignment wins.
pub(crate) fn pretty_name(os_release: &str) -> Option<String> {
    os_release
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("PRETTY_NAME="))
        .map(unquote)
        .filter(|value| !value.is_empty())
}

/// Removes shell-style quotes and backslash escapes as allowed by
/// os-release(5). Variable expansion is not supported there, so none is done.
fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    let inner = match raw.as_bytes() {
        [b'"', .., b'"'] | [b'\'', .., b'\''] => &raw[1..raw.len() - 1],
        _ => raw,
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(escaped) = chars.next() {
                out.push(escaped);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBIAN_13: &str = r#"PRETTY_NAME="Debian GNU/Linux 13 (trixie)"
NAME="Debian GNU/Linux"
VERSION_ID="13"
VERSION="13 (trixie)"
VERSION_CODENAME=trixie
DEBIAN_VERSION_FULL=13.1
ID=debian
HOME_URL="https://www.debian.org/"
"#;

    #[test]
    fn reads_debian_pretty_name() {
        assert_eq!(pretty_name(DEBIAN_13).as_deref(), Some("Debian GNU/Linux 13 (trixie)"));
    }

    #[test]
    fn accepts_unquoted_and_single_quoted_values() {
        assert_eq!(pretty_name("PRETTY_NAME=Plain").as_deref(), Some("Plain"));
        assert_eq!(pretty_name("PRETTY_NAME='Single quoted'").as_deref(), Some("Single quoted"));
    }

    #[test]
    fn handles_escapes() {
        assert_eq!(pretty_name(r#"PRETTY_NAME="Say \"hi\"""#).as_deref(), Some(r#"Say "hi""#));
    }

    #[test]
    fn missing_or_empty_is_none() {
        assert_eq!(pretty_name("NAME=Debian"), None);
        assert_eq!(pretty_name("PRETTY_NAME=\"\""), None);
        assert_eq!(pretty_name(""), None);
    }

    #[test]
    fn ignores_similar_keys() {
        assert_eq!(pretty_name("XPRETTY_NAME=nope\nPRETTY_NAME=yes").as_deref(), Some("yes"));
    }
}
