use zendiloos_kernel::{Secret, constant_time_eq};

/// One-time code that authorizes the initial setup.
///
/// The system shows it only locally (console, journal, a root-only file), so
/// nobody else on the network can complete the setup before the owner.
/// Twelve characters from an alphabet without look-alikes give 60 bits;
/// typed input may use any case, spaces and dashes.
pub struct SetupCode(Secret);

impl SetupCode {
    /// 24 letters without `I` and `O`, and the digits `2` to `9`: 32
    /// characters, so a random byte masked to 5 bits picks one uniformly.
    pub const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    pub const LEN: usize = 12;

    pub fn from_random_bytes(bytes: [u8; Self::LEN]) -> Self {
        let code = bytes.iter().map(|b| char::from(Self::ALPHABET[usize::from(b & 31)])).collect::<String>();
        Self(Secret::new(code))
    }

    /// Reads a stored or typed code; `None` if it cannot be a setup code.
    pub fn parse(input: &str) -> Option<Self> {
        let code: String = input
            .chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let valid = code.len() == Self::LEN && code.bytes().all(|b| Self::ALPHABET.contains(&b));
        valid.then(|| Self(Secret::new(code)))
    }

    pub fn matches(&self, input: &str) -> bool {
        Self::parse(input)
            .is_some_and(|other| constant_time_eq(self.0.expose().as_bytes(), other.0.expose().as_bytes()))
    }

    /// The code in groups of four, `ABCD-EFGH-JKLM`, for showing to the
    /// owner. Deliberately not `Display`, so it cannot be logged by accident.
    pub fn reveal(&self) -> String {
        let code = self.0.expose();
        format!("{}-{}-{}", &code[0..4], &code[4..8], &code[8..12])
    }
}

impl std::fmt::Debug for SetupCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SetupCode(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_random_bytes_onto_the_alphabet() {
        let code = SetupCode::from_random_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 31 + 32]);
        assert_eq!(code.reveal(), "ABCD-EFGH-JKL9");
    }

    #[test]
    fn matching_tolerates_case_spaces_and_dashes() {
        let code = SetupCode::parse("ABCD-EFGH-JKLM").unwrap();
        assert!(code.matches("ABCD-EFGH-JKLM"));
        assert!(code.matches("abcd efgh jklm"));
        assert!(code.matches("abcdefghjklm"));
        assert!(!code.matches("ABCD-EFGH-JKLN"));
        assert!(!code.matches("ABCD-EFGH-JKL"));
        assert!(!code.matches(""));
    }

    #[test]
    fn rejects_characters_outside_the_alphabet() {
        assert!(SetupCode::parse("ABCD-EFGH-JKLO").is_none(), "O looks like 0");
        assert!(SetupCode::parse("ABCD-EFGH-JKL1").is_none(), "1 looks like I");
    }

    #[test]
    fn debug_output_is_redacted() {
        assert_eq!(format!("{:?}", SetupCode::parse("ABCD-EFGH-JKLM").unwrap()), "SetupCode(***)");
    }
}
