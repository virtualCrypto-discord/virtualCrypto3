//! Deliveries to an application, through the proxy that forwards them.
//!
//! What is here so far is the request as a value: the body, the timestamp, the
//! destination and the signature. Sending it needs the proxy's URL and the client
//! certificate it demands, which is configuration this service does not have yet.
//!
//! ## What an application is told to expect
//!
//! Its own documentation — `docs/api/Webhook.md` — promises the same thing this
//! service verifies on its own interaction endpoint: an ed25519 signature over
//! the timestamp and the body concatenated as bytes, in `X-Signature-Ed25519` as
//! **hex, lowercase**, and the timestamp in `X-Signature-Timestamp`.
//!
//! The key is the **application's**, not this service's. Each application
//! carries a keypair — `applications.public_key` and `private_key`, both NOT NULL
//! — and it verifies what it is sent with the public half.

use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;

/// The proxy's destination header: where it should forward this.
pub const FORWARD: &str = "X-Forward";
/// The proxy's answer header: the status the destination replied with.
pub const STATUS: &str = "X-Status";

/// A request to the proxy, built rather than sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub body: String,
    pub signature: String,
    pub timestamp: i64,
    pub forward: String,
}

/// `execute_raw/5`'s request.
///
/// The timestamp is taken as an argument rather than read here, because a
/// signature over a value nobody wrote down is a signature nobody can check.
pub fn delivery(event: &Value, private_key: &[u8; 32], forward: &str, timestamp: i64) -> Delivery {
    let body = event.to_string();
    let signature =
        SigningKey::from_bytes(private_key).sign(format!("{timestamp}{body}").as_bytes());

    Delivery {
        body,
        // Lowercase hex, which is what the documentation says and what an
        // application comparing against a fixed string will have used.
        signature: signature
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        timestamp,
        forward: forward.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use serde_json::json;

    /// A keypair, as the application would hold it: the seed it signs with and
    /// the public half it verifies with.
    fn keypair() -> ([u8; 32], VerifyingKey) {
        let signing = SigningKey::from_bytes(&[7u8; 32]);

        (signing.to_bytes(), signing.verifying_key())
    }

    fn signed(event: &Value) -> (Delivery, VerifyingKey) {
        let (seed, verifying) = keypair();

        (
            delivery(event, &seed, "https://app.example/hook", 1_700_000_000),
            verifying,
        )
    }

    /// The check an application makes, from its own documentation: the timestamp
    /// and the body concatenated, verified with the public key.
    #[test]
    fn an_application_can_verify_what_it_is_sent() {
        let event = json!({ "type": 2, "data": [] });
        let (delivery, verifying) = signed(&event);

        let signature: [u8; 64] = delivery
            .signature
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .expect("32 bytes");

        let message = format!("{}{}", delivery.timestamp, delivery.body);

        assert!(
            verifying
                .verify(message.as_bytes(), &Signature::from_bytes(&signature))
                .is_ok()
        );
    }

    /// Signed over the timestamp *and* the body, so neither can be changed
    /// without the other's signature failing.
    #[test]
    fn neither_the_body_nor_the_timestamp_can_be_changed() {
        let event = json!({ "type": 2, "data": [] });
        let (delivery, verifying) = signed(&event);

        let signature: [u8; 64] = delivery
            .signature
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>()
            .try_into()
            .expect("32 bytes");

        let tampered = format!("{}{}", delivery.timestamp, r#"{"type":1}"#);

        assert!(
            verifying
                .verify(tampered.as_bytes(), &Signature::from_bytes(&signature))
                .is_err()
        );
    }

    #[test]
    fn the_destination_travels_with_the_request() {
        let event = json!({ "type": 1 });
        let (delivery, _) = signed(&event);

        assert_eq!(delivery.forward, "https://app.example/hook");
        assert_eq!(delivery.timestamp, 1_700_000_000);
        assert_eq!(delivery.body, event.to_string());
        assert_eq!(delivery.signature.len(), 128, "64 bytes, hex");
        assert!(
            delivery
                .signature
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{}",
            delivery.signature
        );
    }
}
