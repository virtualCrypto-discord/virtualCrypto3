//! RFC 7636 S256, including RFC 9700's verifier-without-challenge rejection.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Challenge(String);

impl Challenge {
    pub fn from_request(
        challenge: Option<&str>,
        method: Option<&str>,
        required: bool,
    ) -> Result<Option<Self>, &'static str> {
        let Some(challenge) = challenge else {
            return if method.is_some() {
                Err("invalid_code_challenge")
            } else if required {
                Err("code_challenge_required")
            } else {
                Ok(None)
            };
        };
        if method != Some("S256") {
            return Err("invalid_code_challenge_method");
        }
        if challenge.len() != 43
            || URL_SAFE_NO_PAD
                .decode(challenge)
                .map_or(true, |bytes| bytes.len() != 32)
        {
            return Err("invalid_code_challenge");
        }
        Ok(Some(Self(challenge.to_owned())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn verify(challenge: Option<&str>, verifier: Option<&str>) -> bool {
    match (challenge, verifier) {
        (None, None) => true,
        (Some(challenge), Some(verifier)) => {
            (43..=128).contains(&verifier.len())
                && verifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                && crate::application::secrets_match(
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                    challenge,
                )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    #[test]
    fn rfc7636_vector_and_downgrade_protection() {
        assert!(Challenge::from_request(Some(CHALLENGE), Some("S256"), true).is_ok());
        assert!(verify(Some(CHALLENGE), Some(VERIFIER)));
        assert!(!verify(Some(CHALLENGE), Some(&"x".repeat(43))));
        assert!(!verify(Some(CHALLENGE), None));
        assert!(!verify(None, Some(VERIFIER)));
        assert!(verify(None, None));
    }

    #[test]
    fn only_explicit_s256_with_a_valid_digest_is_accepted() {
        assert_eq!(Challenge::from_request(None, None, false), Ok(None));
        assert!(Challenge::from_request(None, None, true).is_err());
        assert!(Challenge::from_request(None, Some("S256"), false).is_err());
        for method in [None, Some("plain"), Some("s256"), Some("unknown")] {
            assert!(Challenge::from_request(Some(CHALLENGE), method, false).is_err());
        }
        for value in [
            "",
            "short",
            &"x".repeat(128),
            &".".repeat(43),
            &format!("{CHALLENGE}="),
        ] {
            assert!(Challenge::from_request(Some(value), Some("S256"), false).is_err());
        }
    }

    #[test]
    fn verifier_grammar_is_enforced_even_if_the_hash_matches() {
        for (value, expected) in [
            ("x".repeat(42), false),
            ("x".repeat(43), true),
            ("x".repeat(128), true),
            ("x".repeat(129), false),
            ("-._~".repeat(16), true),
            ("é".repeat(43), false),
            (" ".repeat(43), false),
            ("+".repeat(43), false),
        ] {
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()));
            assert_eq!(verify(Some(&challenge), Some(&value)), expected);
        }
    }
}
