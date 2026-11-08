//! Two-factor sign-in: a time-based one-time code (TOTP, RFC 6238) from an
//! authenticator app after the password, or one of ten recovery codes.
//!
//! The arithmetic of a code (HMAC-SHA-1) and randomness are the
//! [`OneTimeCodes`](crate::application::ports::OneTimeCodes) port's; here
//! is what a code may look like, which time steps it may belong to, that
//! each is taken once, and what is kept of an account's second factor.

use std::fmt;

use zendiloos_kernel::{Timestamp, constant_time_eq};

use super::{AccountId, AccountName};

/// Seconds a code is valid for (RFC 6238's default, what every app uses).
pub const PERIOD_SECONDS: u64 = 30;
/// Digits of a code.
pub const DIGITS: usize = 6;
/// Bytes of a secret: 160 bits, as RFC 4226 recommends for HMAC-SHA-1.
pub const SECRET_BYTES: usize = 20;
/// Steps before and after the current one a code may come from: phones
/// whose clocks are a little off, a code typed as it changed.
pub const DRIFT_STEPS: u64 = 1;
/// How many recovery codes an account gets at a time.
pub const RECOVERY_CODES: usize = 10;
/// Characters of a recovery code, without the dash shown in the middle.
pub const RECOVERY_CODE_LEN: usize = 10;
/// What recovery codes are made of: no 0/o, 1/l/i, nothing to mistake.
pub const RECOVERY_ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";

/// The secret an authenticator app and the NAS share. Never shown in logs.
#[derive(Clone, PartialEq, Eq)]
pub struct TotpSecret([u8; SECRET_BYTES]);

impl TotpSecret {
    pub const fn from_bytes(bytes: [u8; SECRET_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn bytes(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// As apps take it typed in, and as it is stored: RFC 4648 base32
    /// without padding, 32 characters.
    pub fn to_base32(&self) -> String {
        base32_encode(&self.0)
    }

    /// The secret [`to_base32`](Self::to_base32) wrote; `None` for
    /// anything else.
    pub fn from_base32(text: &str) -> Option<Self> {
        let bytes = base32_decode(text)?;
        Some(Self(bytes.try_into().ok()?))
    }
}

impl fmt::Debug for TotpSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TotpSecret(…)")
    }
}

/// The time step `now` falls in.
pub fn time_step(now: Timestamp) -> u64 {
    u64::try_from(now.unix_seconds()).unwrap_or(0) / PERIOD_SECONDS
}

/// The address an authenticator app reads from the QR code. Account names
/// are lowercase letters, digits, `_` and `-`, which need no escaping.
pub fn otpauth_uri(name: &AccountName, secret: &TotpSecret) -> String {
    format!(
        "otpauth://totp/zendiloOS:{name}?secret={}&issuer=zendiloOS&algorithm=SHA1&digits={DIGITS}&period={PERIOD_SECONDS}",
        secret.to_base32()
    )
}

/// A code as someone typed it, told apart by its form: spaces and dashes
/// do not count, case does not either.
#[derive(Clone, PartialEq, Eq)]
pub enum TypedCode {
    /// Six digits from the app.
    OneTime(String),
    /// A recovery code, normalized: ten characters of
    /// [`RECOVERY_ALPHABET`], no dash.
    Recovery(String),
}

impl fmt::Debug for TypedCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OneTime(_) => "OneTime(…)",
            Self::Recovery(_) => "Recovery(…)",
        })
    }
}

impl TypedCode {
    /// `None` for what is neither.
    pub fn parse(text: &str) -> Option<Self> {
        let kept: String =
            text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_lowercase();
        if kept.len() == DIGITS && kept.bytes().all(|b| b.is_ascii_digit()) {
            Some(Self::OneTime(kept))
        } else if kept.len() == RECOVERY_CODE_LEN && kept.bytes().all(|b| RECOVERY_ALPHABET.contains(&b)) {
            Some(Self::Recovery(kept))
        } else {
            None
        }
    }
}

/// A recovery code as shown once: `abcde-fghjk`.
pub fn show_recovery_code(code: &str) -> String {
    let (first, second) = code.split_at(code.len() / 2);
    format!("{first}-{second}")
}

/// What is kept of a recovery code: a digest of its normalized form, never
/// the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryHash(String);

impl RecoveryHash {
    pub fn new(digest: impl Into<String>) -> Self {
        Self(digest.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An account's second factor as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoFactor {
    pub account_id: AccountId,
    /// Confirmed: sign-ins ask for a code.
    pub secret: Option<TotpSecret>,
    /// Set up, waiting for its first code; replaces `secret` once confirmed.
    pub pending: Option<TotpSecret>,
    /// The last time step a code was taken for: each is taken once.
    pub last_step: Option<u64>,
    /// The recovery codes not used yet.
    pub recovery: Vec<RecoveryHash>,
    /// When it was turned on, which tells this setup from a later one.
    pub enabled_at: Option<Timestamp>,
}

impl TwoFactor {
    pub fn new(account_id: AccountId) -> Self {
        Self {
            account_id,
            secret: None,
            pending: None,
            last_step: None,
            recovery: Vec::new(),
            enabled_at: None,
        }
    }

    /// Sign-ins of the account ask for a second factor.
    pub fn is_on(&self) -> bool {
        self.secret.is_some()
    }
}

/// The time step `code` belongs to for a secret whose codes `code_at`
/// gives: one within [`DRIFT_STEPS`] of `now`, later than `last_step`, as
/// a step is taken once. `None` if there is none.
pub fn matching_step(
    code: &str,
    now: u64,
    last_step: Option<u64>,
    code_at: impl Fn(u64) -> String,
) -> Option<u64> {
    let mut found = None;
    for step in now.saturating_sub(DRIFT_STEPS)..=now.saturating_add(DRIFT_STEPS) {
        if last_step.is_some_and(|last| step <= last) {
            continue;
        }
        // Every step is compared, and each in constant time: how long it
        // takes tells nothing about which digits were right.
        if constant_time_eq(code_at(step).as_bytes(), code.as_bytes()) && found.is_none() {
            found = Some(step);
        }
    }
    found
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.trim_end_matches('=').bytes() {
        let value = BASE32.iter().position(|&b| b == c.to_ascii_uppercase())?;
        buffer = (buffer << 5) | u32::try_from(value).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_go_to_base32_and_back() {
        // RFC 4648's examples, then a secret of twenty bytes.
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("MZXW6YTBOI======").unwrap(), b"foobar");
        let secret = TotpSecret::from_bytes(*b"12345678901234567890");
        assert_eq!(secret.to_base32(), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(TotpSecret::from_base32(&secret.to_base32()), Some(secret.clone()));
        assert_eq!(TotpSecret::from_base32("gezdgnbvgy3tqojqgezdgnbvgy3tqojq"), Some(secret));
        assert_eq!(TotpSecret::from_base32("MZXW6YTBOI"), None, "not twenty bytes");
        assert_eq!(TotpSecret::from_base32("0189"), None, "not base32");
        assert_eq!(format!("{:?}", TotpSecret::from_bytes([7; 20])), "TotpSecret(…)");
    }

    #[test]
    fn the_uri_names_the_account_and_the_issuer() {
        let name = AccountName::parse("alice").unwrap();
        let uri = otpauth_uri(&name, &TotpSecret::from_bytes(*b"12345678901234567890"));
        assert_eq!(
            uri,
            "otpauth://totp/zendiloOS:alice?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=zendiloOS&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn codes_are_told_apart_by_their_form() {
        assert_eq!(TypedCode::parse(" 123 456 "), Some(TypedCode::OneTime("123456".into())));
        assert_eq!(TypedCode::parse("ABCDE-FGHJK"), Some(TypedCode::Recovery("abcdefghjk".into())));
        assert_eq!(TypedCode::parse("abcde fghjk"), Some(TypedCode::Recovery("abcdefghjk".into())));
        assert_eq!(TypedCode::parse("12345"), None);
        assert_eq!(TypedCode::parse("abcde-fghjl"), None, "l is not in the alphabet");
        assert_eq!(TypedCode::parse(""), None);
        assert_eq!(format!("{:?}", TypedCode::parse("123456").unwrap()), "OneTime(…)");
        assert_eq!(show_recovery_code("abcdefghjk"), "abcde-fghjk");
        assert_eq!(RECOVERY_ALPHABET.len(), 31);
    }

    #[test]
    fn steps_follow_the_clock() {
        assert_eq!(time_step(Timestamp::from_unix_seconds(59)), 1);
        assert_eq!(time_step(Timestamp::from_unix_seconds(1_111_111_109)), 37_037_036);
    }

    #[test]
    fn a_code_matches_its_step_or_one_beside_it_once() {
        let code_at = |step: u64| format!("{:06}", step % 1_000_000);
        assert_eq!(matching_step("000100", 100, None, code_at), Some(100));
        assert_eq!(matching_step("000099", 100, None, code_at), Some(99), "a step behind");
        assert_eq!(matching_step("000101", 100, None, code_at), Some(101), "a step ahead");
        assert_eq!(matching_step("000098", 100, None, code_at), None, "too old");
        assert_eq!(matching_step("000102", 100, None, code_at), None, "too new");
        assert_eq!(matching_step("000100", 100, Some(100), code_at), None, "taken already");
        assert_eq!(matching_step("000101", 100, Some(100), code_at), Some(101));
        assert_eq!(matching_step("000099", 100, Some(99), code_at), None, "an older step after a newer");
    }

    #[test]
    fn a_new_second_factor_is_off() {
        let fresh = TwoFactor::new(AccountId::new(3));
        assert!(!fresh.is_on());
        let on = TwoFactor { secret: Some(TotpSecret::from_bytes([1; 20])), ..fresh };
        assert!(on.is_on());
        assert_eq!(RecoveryHash::new("ab").as_str(), "ab");
    }
}
