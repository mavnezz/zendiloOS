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

#[cfg(test)]
mod tests {
    use super::*;

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
