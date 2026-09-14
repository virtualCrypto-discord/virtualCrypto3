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
}

impl Transport for Proxy {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }

    fn endpoint<'a>(&'a self, _delivery: &'a Delivery) -> &'a str {
        &self.url
    }

    /// The status the *destination* gave, which the proxy reports in a header: the proxy's own
    /// answer is a `200` whatever the destination did, and a caller that cannot tell those apart
    /// cannot act on either. A proxy that answered without saying is a proxy that did not answer.
    fn status(&self, response: &reqwest::Response) -> Option<u16> {
        response
            .headers()
            .get(STATUS)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    }
}

/// Straight at the application, for the machines that have no proxy.
///
/// A development machine has none, and a registration that names a `webhook_url` has to be
/// verifiable there or the application flow cannot be exercised end to end. What it needs is the
/// same request sent to the address the delivery names, and the status the application answers
/// with rather than one a proxy reported.
pub struct Direct {
    http: reqwest::Client,
}

impl Default for Direct {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl Transport for Direct {
    fn http(&self) -> &reqwest::Client {
        &self.http
    }

    fn endpoint<'a>(&'a self, delivery: &'a Delivery) -> &'a str {
        &delivery.forward
    }

    fn status(&self, response: &reqwest::Response) -> Option<u16> {
        Some(response.status().as_u16())
    }
}

/// Where a webhook goes, which is the only thing that differs between the two ways of sending one.
///
/// The request is the same either way — the same two signature headers over the same body — so it
/// is written once, in [`send`]. A transport answers two questions about it: *where* it goes, and
/// *where the status is read from*. The proxy relays to a destination it names in a header and
/// reports the answer in another; a direct call is answered by the application itself.
/// The two bounds are the ones a delivery needs rather than decoration: the notifier spawns the
/// send, so a transport that is not `Send + Sync` cannot be used from it at all.
pub trait Transport: Send + Sync {
    fn http(&self) -> &reqwest::Client;
    fn endpoint<'a>(&'a self, delivery: &'a Delivery) -> &'a str;
    fn status(&self, response: &reqwest::Response) -> Option<u16>;
}

/// Send a delivery, however this service reaches applications.
pub async fn send(
    transport: &dyn Transport,
    delivery: &Delivery,
) -> Result<Option<(u16, Value)>, reqwest::Error> {
    let response = transport
        .http()
        .post(transport.endpoint(delivery))
        .header("X-Signature-Ed25519", &delivery.signature)
        .header("X-Signature-Timestamp", delivery.timestamp.to_string())
        // The proxy needs to be told where to relay to. Sending it to an application that does not
        // read it costs one header, and a branch here would cost the shape this is here to keep.
        .header(FORWARD, &delivery.forward)
        .body(delivery.body.clone())
        .send()
        .await?;

    // The status first, because the body consumes the response and the handshake needs both.
    let status = transport.status(&response);
    let body = response.json::<Value>().await.ok();

    Ok(status.zip(body))
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
    /// The proxy is taken as an `Arc` rather than built into one, because the
    /// state holds the same one: the handshake a registration performs and the
    /// deliveries an application receives go through one proxy, and there is no
    /// reason for two.
    pub fn new(pool: sqlx::PgPool, proxy: std::sync::Arc<Proxy>) -> Self {
        Self { pool, proxy }
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

    if let Err(error) = send(proxy, &delivery).await {
        tracing::warn!(application_id, %error, "could not reach the webhook proxy");
    }
}

/// The outcome of the handshake an application must pass to be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handshake {
    /// It answers a real signature and refuses a false one.
    Passed,
    /// It answered, and something about the answer was wrong.
    Failed,
    /// The proxy never said what the application answered.
    Unreachable,
}

/// `_verify/3`'s two answers, decided.
///
/// Two requests, and each asks a different question. The first is a PING signed
/// with the application's real key, and must come back `200` carrying the PING
/// back — an application that answers webhooks at all. The second is the same
/// PING signed with a keypair generated for the occasion, and must come back
/// `401` — an application that *checks* them.
///
/// That second request is the whole point of doing this twice. An application
/// that answers `200` to both is not verifying anything and would accept a
/// forgery from anyone; one that answers `401` to both cannot answer at all. Both
/// look fine to a single request.
pub fn handshake(real: Option<(u16, &Value)>, wrong_key: Option<(u16, &Value)>) -> Handshake {
    match (real, wrong_key) {
        (Some((200, body)), Some((401, _))) if body["type"] == PING => Handshake::Passed,
        (Some(_), Some(_)) => Handshake::Failed,
        _ => Handshake::Unreachable,
    }
}

/// A keypair generated for one handshake, and used for nothing else.
///
/// The second request of a handshake is signed with this rather than with the
/// application's key, because its whole purpose is to be a signature the
/// application should refuse. It is generated rather than fixed: a constant one
/// would let an application pass by recognising it instead of by verifying.
pub fn fresh_keypair() -> [u8; 32] {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("the operating system's randomness");

    seed
}

/// The handshake an application must pass before it is registered, and
/// periodically afterwards.
///
/// Two requests, as the Elixir sends them. The second is signed with a keypair
/// generated here and used for nothing else, because its whole purpose is to be a
/// signature the application should refuse.
pub async fn verify(
    transport: &dyn Transport,
    url: &str,
    private_key: &[u8; 32],
    at: i64,
) -> Handshake {
    let real = send_ping(transport, url, private_key, at).await;
    let wrong = send_ping(transport, url, &fresh_keypair(), at).await;

    handshake(
        real.as_ref().map(|(status, body)| (*status, body)),
        wrong.as_ref().map(|(status, body)| (*status, body)),
    )
}

/// One PING, and what the application answered through the proxy.
async fn send_ping(
    transport: &dyn Transport,
    url: &str,
    private_key: &[u8; 32],
    at: i64,
) -> Option<(u16, Value)> {
    let ping = serde_json::json!({ "type": PING });

    send(transport, &delivery(&ping, private_key, url, at))
        .await
        .ok()
        .flatten()
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

    /// An application, as far as a handshake can tell: it verifies a signature the way its own
    /// documentation says, answers the PING it believes, and refuses the one it does not.
    async fn application(public: VerifyingKey) -> String {
        let hook = move |headers: axum::http::HeaderMap, body: String| async move {
            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned()
            };

            let signature = header("X-Signature-Ed25519");
            let timestamp = header("X-Signature-Timestamp");

            let bytes: Option<[u8; 64]> = (0..signature.len() / 2)
                .map(|pair| u8::from_str_radix(&signature[pair * 2..pair * 2 + 2], 16).ok())
                .collect::<Option<Vec<_>>>()
                .and_then(|bytes| bytes.try_into().ok());

            let message = format!("{timestamp}{body}");

            let believed = bytes
                .map(|bytes| Signature::from_bytes(&bytes))
                .is_some_and(|signature| public.verify(message.as_bytes(), &signature).is_ok());

            if believed {
                (
                    axum::http::StatusCode::OK,
                    axum::Json(json!({ "type": PING })),
                )
            } else {
                (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({})))
            }
        };

        let app = axum::Router::new().route("/hook", axum::routing::post(hook));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("the address");

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        format!("http://{address}/hook")
    }

    /// No proxy is a direct handshake, which is the whole point of this: a machine with no proxy
    /// could not register an application that names a webhook at all, and so could not exercise
    /// the application flow.
    #[tokio::test]
    async fn a_handshake_without_a_proxy_goes_straight_at_the_webhook() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let url = application(signing.verifying_key()).await;

        assert_eq!(
            verify(&Direct::default(), &url, &signing.to_bytes(), 1_700_000_000).await,
            Handshake::Passed,
            "{url}"
        );
    }

    /// And a delivery goes there too, rather than through anything: the same request the proxy
    /// would relay, answered by the application itself.
    #[tokio::test]
    async fn a_delivery_without_a_proxy_is_answered_by_the_application() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let url = application(signing.verifying_key()).await;

        let sent = send(
            &Direct::default(),
            &delivery(&json!({ "type": PING }), &signing.to_bytes(), &url, 1),
        )
        .await
        .expect("a request");

        assert_eq!(sent.map(|(status, _)| status), Some(200));
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

    fn ping_body() -> Value {
        json!({ "type": PING })
    }

    #[test]
    fn an_application_that_verifies_passes() {
        assert_eq!(
            handshake(Some((200, &ping_body())), Some((401, &json!({})))),
            Handshake::Passed
        );
    }

    /// The case the second request exists for: an application that answers
    /// everything looks fine to one request and is not verifying anything.
    #[test]
    fn an_application_that_answers_everything_fails() {
        assert_eq!(
            handshake(Some((200, &ping_body())), Some((200, &ping_body()))),
            Handshake::Failed
        );
    }

    /// And an application that refuses everything cannot answer at all, which is
    /// not the same as being careful.
    #[test]
    fn an_application_that_refuses_everything_fails() {
        assert_eq!(
            handshake(Some((401, &json!({}))), Some((401, &json!({})))),
            Handshake::Failed
        );
    }

    /// It has to echo the PING: a 200 is not an answer to the question.
    #[test]
    fn a_200_that_is_not_the_ping_fails() {
        assert_eq!(
            handshake(Some((200, &json!({ "type": 2 }))), Some((401, &json!({})))),
            Handshake::Failed
        );
    }

    /// A proxy that did not say is not an application that failed.
    #[test]
    fn no_answer_is_not_a_failure() {
        assert_eq!(
            handshake(None, Some((401, &json!({})))),
            Handshake::Unreachable
        );
        assert_eq!(
            handshake(Some((200, &ping_body())), None),
            Handshake::Unreachable
        );
    }

    /// The property the second request rests on: what it sends cannot be verified
    /// with the application's own public key, so an application that verifies
    /// anything at all will refuse it.
    #[test]
    fn a_fresh_keypair_signs_nothing_the_application_accepts() {
        let application = SigningKey::from_bytes(&[11u8; 32]);
        let wrong = fresh_keypair();

        assert_ne!(wrong, application.to_bytes(), "not the application's key");

        let delivery = delivery(&json!({ "type": PING }), &wrong, "https://app.example", 1);
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
            application
                .verifying_key()
                .verify(message.as_bytes(), &Signature::from_bytes(&signature))
                .is_err(),
            "an application that verifies would refuse this"
        );
    }

    /// Two handshakes do not use the same key, or the second request would be
    /// something an application could recognise rather than verify.
    #[test]
    fn two_fresh_keypairs_are_not_the_same_key() {
        assert_ne!(fresh_keypair(), fresh_keypair());
    }
}
