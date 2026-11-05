use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use zendiloos_identity::application::ports::{IssuedToken, SessionTokens};
use zendiloos_identity::domain::TokenHash;
use zendiloos_kernel::Secret;

/// 256-bit tokens from the operating system's random source, URL-safe
/// base64 encoded (43 characters).
#[derive(Debug, Default)]
pub struct RandomSessionTokens;

pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    // getrandom(2) does not fail on Linux once the kernel's pool is
    // initialized, which happens early in boot.
    getrandom::fill(&mut bytes).expect("the operating system's random source is unavailable");
    bytes
}

fn random_token() -> Secret {
    Secret::new(URL_SAFE_NO_PAD.encode(random_bytes::<32>()))
}

impl SessionTokens for RandomSessionTokens {
    fn issue(&self) -> IssuedToken {
        let token = random_token();
        IssuedToken { hash: self.digest(&token), token }
    }

    fn digest(&self, token: &Secret) -> TokenHash {
        TokenHash::new(Sha256::digest(token.expose().as_bytes()).into())
    }

    fn csrf_token(&self) -> Secret {
        random_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_unique_and_url_safe() {
        let first = RandomSessionTokens.issue();
        let second = RandomSessionTokens.issue();
        let value = first.token.expose();
        assert_eq!(value.len(), 43);
        assert!(value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(value, second.token.expose());
        assert_ne!(RandomSessionTokens.csrf_token().expose(), RandomSessionTokens.csrf_token().expose());
    }

    #[test]
    fn the_stored_hash_is_the_sha256_of_the_token() {
        let issued = RandomSessionTokens.issue();
        assert_eq!(issued.hash, RandomSessionTokens.digest(&issued.token));
        // Known answer: SHA-256("abc").
        let digest = RandomSessionTokens.digest(&Secret::new("abc"));
        assert_eq!(digest.as_bytes()[..4], [0xba, 0x78, 0x16, 0xbf]);
    }
}
