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

/// A client for the proxy, and the certificate it demands.
///
/// The worker is protected by **mutual** TLS: it presents a certificate and
/// requires one. A client without the certificate is not a client it will answer,
/// which is why the identity is built here rather than left to a caller.
pub struct Proxy {
    http: reqwest::Client,
    url: String,
}

impl Proxy {
    /// The certificate and its key, as PEM, which is how `reqwest` takes an
    /// identity: both together in one buffer.
    pub fn new(
        url: impl Into<String>,
        certificate: &[u8],
        key: &[u8],
    ) -> Result<Self, reqwest::Error> {
        let mut pem = certificate.to_vec();
        pem.push(b'\n');
        pem.extend_from_slice(key);

        let http = reqwest::Client::builder()
            .identity(reqwest::Identity::from_pem(&pem)?)
            .build()?;

        Ok(Self {
            http,
            url: url.into(),
        })
    }

    /// Send a delivery, answering with the status the *destination* gave.
    ///
    /// `None` is a proxy that did not say: the status travels in `X-Status`
    /// because the proxy's own answer is a `200` whatever the destination did,
    /// and a caller that cannot tell those apart cannot act on either.
    pub async fn send(&self, delivery: &Delivery) -> Result<Option<u16>, reqwest::Error> {
        let response = self
            .http
            .post(&self.url)
            .header("X-Signature-Ed25519", &delivery.signature)
            .header("X-Signature-Timestamp", delivery.timestamp.to_string())
            .header(FORWARD, &delivery.forward)
            .body(delivery.body.clone())
            .send()
            .await?;

        Ok(response
            .headers()
            .get(STATUS)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok()))
    }
}

/// The event body for a claim update: type 2, with the events as its data.
///
/// Type 1 is the PING the handshake sends, and an application must ignore types
/// it does not know — which is what its documentation says, and what makes adding
/// a third type safe later.
pub fn claim_update_body(events: &[Value]) -> Value {
    serde_json::json!({ "type": 2, "data": events })
}

/// The type of the handshake's event.
pub const PING: i64 = 1;

/// The delivery, wired up: an application's events, sent the way its own
/// documentation says they will be.
#[derive(Clone)]
pub struct WebhookNotifier {
    pool: sqlx::PgPool,
    proxy: std::sync::Arc<Proxy>,
}

impl WebhookNotifier {
    pub fn new(pool: sqlx::PgPool, proxy: Proxy) -> Self {
        Self {
            pool,
            proxy: std::sync::Arc::new(proxy),
        }
    }
}

impl vc_core::notification::Notifier for WebhookNotifier {
    /// Fire and forget, as `Task.start/1` does: an application that is slow to
    /// answer must not slow the claim that caused the event, and an application
    /// that never answers must not stop it.
    fn notify_claim_update(&self, claimant_id: i32, events: &[Value]) {
        let pool = self.pool.clone();
        let proxy = std::sync::Arc::clone(&self.proxy);
        let events = events.to_vec();

        tokio::spawn(async move {
            send_claim_update(&pool, &proxy, claimant_id, &events).await;
        });
    }
}

/// One delivery, if there is anywhere to deliver it.
async fn send_claim_update(pool: &sqlx::PgPool, proxy: &Proxy, claimant_id: i32, events: &[Value]) {
    // The application the claimant acts for, if it acts for one. An account that
    // is nobody's application has nowhere to be told, which is the Elixir's
    // `:nop` rather than a failure.
    let Some(application_id) = vc_core::user::application_id(pool, claimant_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    let Some(webhook) = vc_core::application::webhook_data(pool, application_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };

    let Some(url) = webhook.webhook_url.as_deref() else {
        return;
    };

    // A key that is not thirty-two bytes is not an ed25519 seed, and signing with
    // a guess would produce something the application refuses.
    let Ok(private_key) = <[u8; 32]>::try_from(webhook.private_key.as_slice()) else {
        tracing::warn!(
            application_id,
            "the application's private key is not 32 bytes"
        );

        return;
    };

    let body = claim_update_body(events);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default();

    let delivery = delivery(&body, &private_key, url, now);

    if let Err(error) = proxy.send(&delivery).await {
        tracing::warn!(application_id, %error, "could not reach the webhook proxy");
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

    /// The worker demands a certificate, so a client built without one that
    /// parses is not a client — and it fails where it is built rather than at the
    /// first delivery.
    #[test]
    fn a_proxy_needs_a_certificate_it_can_use() {
        let built = Proxy::new("https://proxy.example/", b"not a certificate", b"not a key");

        assert!(
            built.is_err(),
            "a certificate that does not parse was accepted"
        );
    }

    #[test]
    fn a_claim_update_says_it_is_one() {
        let body = claim_update_body(&[json!({ "id": 1, "status": "approved" })]);

        assert_eq!(body["type"], 2);
        assert_eq!(body["data"][0]["status"], "approved");
        assert_ne!(body["type"], PING, "which is the handshake's");
    }
}
