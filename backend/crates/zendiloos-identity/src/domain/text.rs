/// A short free text such as a full name or a description: trimmed, empty
/// means absent, a length limit and no control characters, so it renders on
/// one line wherever it is shown.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the {field} must be at most {max} characters, without line breaks")]
pub struct InvalidText {
    pub field: &'static str,
    pub max: usize,
}

pub const MAX_FULL_NAME_CHARS: usize = 100;
pub const MAX_DESCRIPTION_CHARS: usize = 200;

pub fn optional_text(
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<Option<String>, InvalidText> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(InvalidText { field, max });
    }
    Ok(Some(value.to_owned()))
}

pub fn full_name(value: Option<&str>) -> Result<Option<String>, InvalidText> {
    optional_text("full name", value, MAX_FULL_NAME_CHARS)
}

pub fn description(value: Option<&str>) -> Result<Option<String>, InvalidText> {
    optional_text("description", value, MAX_DESCRIPTION_CHARS)
}

pub const MAX_EMAIL_CHARS: usize = 254;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not an e-mail address")]
pub struct InvalidEmail;

/// An e-mail address, as far as it can be told without sending mail: one
/// `@` with something on both sides and a dot in the domain, no spaces.
pub fn email(value: Option<&str>) -> Result<Option<String>, InvalidEmail> {
    let invalid = InvalidEmail;
    let Some(value) = optional_text("e-mail address", value, MAX_EMAIL_CHARS).map_err(|_| InvalidEmail)?
    else {
        return Ok(None);
    };
    let valid = value.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
    }) && value.matches('@').count() == 1
        && !value.contains(char::is_whitespace);
    if valid { Ok(Some(value)) } else { Err(invalid) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_addresses_look_like_ones() {
        assert_eq!(email(Some(" anna@example.org ")), Ok(Some("anna@example.org".to_owned())));
        assert_eq!(email(Some("")), Ok(None));
        for bad in ["anna", "anna@", "@example.org", "anna@localhost", "a b@example.org", "a@b@c.de", "a@x."]
        {
            assert!(email(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn trims_and_limits() {
        assert_eq!(full_name(Some("  Alice Smith ")), Ok(Some("Alice Smith".to_owned())));
        assert_eq!(full_name(Some("   ")), Ok(None));
        assert_eq!(full_name(None), Ok(None));
        assert_eq!(full_name(Some("Zoë Ångström")), Ok(Some("Zoë Ångström".to_owned())));
        assert!(full_name(Some(&"x".repeat(101))).is_err());
        assert!(full_name(Some("two\nlines")).is_err());
        assert!(description(Some("tab\there")).is_err());
    }
}
