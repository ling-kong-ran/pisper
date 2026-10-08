//! PKCE utilities ported from upstream `packages/ai/src/auth/oauth/pkce.ts`:
//! a random code verifier (32 bytes of OS entropy, base64url-encoded) and its
//! S256 challenge (SHA-256, base64url without padding), used by the OAuth
//! login flows.

use sha2::{Digest, Sha256};

/// Upstream `base64urlEncode` (pkce.ts:9-15): standard base64 alphabet with
/// `+`/`/` swapped for `-_` and padding stripped — exactly what
/// `btoa(...).replace(...)` produces.
pub(crate) fn base64url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let group = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(ALPHABET[(group >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(group >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(group >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[group as usize & 0x3f] as char);
        }
    }
    encoded
}

/// The S256 challenge for a verifier (upstream pkce.ts:29-31): SHA-256 over
/// the verifier's bytes, base64url-encoded without padding.
fn s256_challenge(verifier: &str) -> String {
    base64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// Upstream `generatePKCE` (pkce.ts:21-34): a random 43-character verifier
/// (32 random bytes, base64url) and its S256 challenge.
pub fn generate_pkce() -> Pkce {
    let verifier_bytes = rand_bytes_32();
    let verifier = base64url_encode(&verifier_bytes);
    let challenge = s256_challenge(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

/// Upstream return shape `{ verifier, challenge }` (pkce.ts:21).
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

/// Upstream `crypto.getRandomValues(new Uint8Array(32))`: 32 bytes of OS
/// entropy from the `rand` thread RNG (CryptoRng-backed, see rand 0.10).
/// The oracle tests inject the capture's deterministic byte stream through
/// [`super::test_entropy`].
fn rand_bytes_32() -> [u8; 32] {
    #[cfg(test)]
    if let Some(bytes) = super::test_entropy::take(32) {
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        return array;
    }
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_encode_matches_rfc4648_vectors_with_url_alphabet_and_no_padding() {
        assert_eq!(base64url_encode(&[]), "");
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
        assert_eq!(base64url_encode(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
        // Standard base64 "u/8=" style cases: `+` and `/` become `-` and
        // `_`, padding is stripped (upstream btoa + replace chain).
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url_encode(&[0xff, 0xff, 0xff]), "____");
        assert_eq!(base64url_encode(&[0xfb]), "-w");
    }

    #[test]
    fn s256_challenge_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            s256_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generate_pkce_produces_a_43_char_url_safe_verifier_and_its_s256_challenge() {
        let pkce = generate_pkce();
        // 32 random bytes -> 43 base64url characters, no padding.
        assert_eq!(pkce.verifier.len(), 43);
        assert!(
            pkce.verifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "verifier must only contain base64url characters: {}",
            pkce.verifier
        );
        // The challenge is the S256 of the returned verifier.
        assert_eq!(pkce.challenge, s256_challenge(&pkce.verifier));
        assert_eq!(pkce.challenge.len(), 43);
    }

    #[test]
    fn generate_pkce_does_not_repeat() {
        let first = generate_pkce();
        let second = generate_pkce();
        assert_ne!(first.verifier, second.verifier);
        assert_ne!(first.challenge, second.challenge);
    }
}
