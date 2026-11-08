use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use zendiloos_identity::application::ports::OneTimeCodes;
use zendiloos_identity::domain::{
    DIGITS, RECOVERY_ALPHABET, RECOVERY_CODE_LEN, RECOVERY_CODES, RecoveryHash, SECRET_BYTES, TotpSecret,
};
use zendiloos_kernel::Secret;

use super::tokens::random_bytes;

/// TOTP (RFC 6238) as authenticator apps compute it: HMAC-SHA-1 over the
/// time step, truncated as HOTP (RFC 4226) does, six digits. Secrets and
/// recovery codes from the operating system's random source; recovery codes
/// kept as their SHA-256.
#[derive(Debug, Default)]
pub struct TotpCodes;

/// HOTP's value before it is cut to its digits (RFC 4226, 5.3).
fn hotp(key: &[u8], counter: u64) -> u32 {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 0x0f);
    u32::from_be_bytes([digest[offset] & 0x7f, digest[offset + 1], digest[offset + 2], digest[offset + 3]])
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| [DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 0x0f)]])
        .map(char::from)
        .collect()
}

/// A recovery code: characters of [`RECOVERY_ALPHABET`], each as likely as
/// the next (bytes past the last whole multiple of its length are drawn again).
fn recovery_code() -> String {
    let len = RECOVERY_ALPHABET.len();
    let limit = 256 - 256 % len;
    let mut code = String::with_capacity(RECOVERY_CODE_LEN);
    while code.len() < RECOVERY_CODE_LEN {
        for byte in random_bytes::<16>() {
            let byte = usize::from(byte);
            if byte < limit && code.len() < RECOVERY_CODE_LEN {
                code.push(char::from(RECOVERY_ALPHABET[byte % len]));
            }
        }
    }
    code
}

impl OneTimeCodes for TotpCodes {
    fn new_secret(&self) -> TotpSecret {
        TotpSecret::from_bytes(random_bytes::<SECRET_BYTES>())
    }

    fn code(&self, secret: &TotpSecret, step: u64) -> String {
        let modulus = 10_u32.pow(u32::try_from(DIGITS).expect("a few digits"));
        format!("{:0width$}", hotp(secret.bytes(), step) % modulus, width = DIGITS)
    }

    fn recovery_codes(&self) -> Vec<Secret> {
        (0..RECOVERY_CODES).map(|_| Secret::new(recovery_code())).collect()
    }

    fn digest(&self, code: &str) -> RecoveryHash {
        RecoveryHash::new(hex(&Sha256::digest(code.as_bytes())))
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_identity::domain::{TypedCode, time_step};
    use zendiloos_kernel::Timestamp;

    use super::*;

    /// The key of the test vectors of RFC 4226 and RFC 6238 (SHA-1).
    const KEY: &[u8; 20] = b"12345678901234567890";

    #[test]
    fn hotp_matches_rfc_4226() {
        let expected = [755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489];
        for (counter, value) in expected.into_iter().enumerate() {
            assert_eq!(hotp(KEY, counter as u64) % 1_000_000, value, "counter {counter}");
        }
    }

    #[test]
    fn totp_matches_rfc_6238() {
        // Appendix B, SHA-1, eight digits; apps show the last six.
        let vectors = [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ];
        let secret = TotpSecret::from_bytes(*KEY);
        for (seconds, eight) in vectors {
            let step = time_step(Timestamp::from_unix_seconds(seconds));
            assert_eq!(format!("{:08}", hotp(KEY, step) % 100_000_000), eight, "at {seconds}");
            assert_eq!(TotpCodes.code(&secret, step), eight[2..], "at {seconds}");
        }
    }

    #[test]
    fn secrets_are_random() {
        let first = TotpCodes.new_secret();
        assert_ne!(first, TotpCodes.new_secret());
        assert_eq!(first.to_base32().len(), 32);
    }

    #[test]
    fn recovery_codes_are_random_and_read_as_recovery_codes() {
        let codes = TotpCodes.recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        for code in &codes {
            assert!(
                matches!(TypedCode::parse(code.expose()), Some(TypedCode::Recovery(c)) if c == code.expose())
            );
        }
        let mut distinct: Vec<&str> = codes.iter().map(Secret::expose).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), RECOVERY_CODES);
    }

    #[test]
    fn recovery_codes_are_kept_as_their_sha256() {
        // Known answer: SHA-256("abc").
        assert_eq!(
            TotpCodes.digest("abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
