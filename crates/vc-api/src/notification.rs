//! Deliveries to an application, through the proxy that forwards them.
//!
//! What is here so far is the request as a value: the body, the timestamp, the
//! destination and the signature. Sending it needs the proxy's URL and the client
//! certificate it demands, which is configuration this service does not have yet.
//!
//! ## What an application is told to expect
//!
//! The same thing this service verifies on its own interaction endpoint: an
//! ed25519 signature over the timestamp and the body concatenated as bytes, in
//! `X-Signature-Ed25519` as **hex, lowercase**, and the timestamp in
//! `X-Signature-Timestamp`.
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

/// Bound each delivery, including connection setup and reading the response body.
const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Send a delivery, however this service reaches applications.
pub async fn send(
    transport: &dyn Transport,
    delivery: &Delivery,
) -> Result<Option<(u16, Value)>, reqwest::Error> {
    let response = transport
        .http()
        .post(transport.endpoint(delivery))
        .timeout(DELIVERY_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
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
    // A refusal needs only its status. A successful PING still has to carry
    // {"type": 1}; a missing or invalid JSON body cannot satisfy that check.
    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(error) if error.is_timeout() => return Err(error),
        Err(_) => Value::Null,
    };

    Ok(status.map(|status| (status, body)))
}

/// The event body for a claim update: type 2, with the events as its data.
///
/// Type 1 is the PING the handshake sends, type 3 a grant decision, and an
/// application must ignore types it does not know — which is what its
/// documentation says, and what makes adding another type safe later.
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
    transport: std::sync::Arc<dyn Transport>,
}

impl WebhookNotifier {
    /// The transport is taken as an `Arc` rather than built here, because the
    /// deliveries are spawned — and as a trait object because reaching an
    /// application is the one thing that differs between deployments: `Proxy`
    /// where a worker is configured, `Direct` where there is none. Both are what
    /// the handshake beside it chooses between, so a service that can verify a
    /// webhook is also one that can deliver to it.
    pub fn new(pool: sqlx::PgPool, transport: std::sync::Arc<dyn Transport>) -> Self {
        Self { pool, transport }
    }
}

impl vc_core::notification::Notifier for WebhookNotifier {
    /// Fire and forget, as `Task.start/1` does: an application that is slow to
    /// answer must not slow the claim that caused the event, and an application
    /// that never answers must not stop it.
    fn notify_claim_update(&self, claimant_id: i32, events: &[Value]) {
        let pool = self.pool.clone();
        let transport = std::sync::Arc::clone(&self.transport);
        let events = events.to_vec();

        tokio::spawn(async move {
            send_claim_update(&pool, &*transport, claimant_id, &events).await;
        });
    }

    /// Fire and forget for the same reason: the guild already decided, and the
    /// application's poll would learn it anyway — this is the ping that saves
    /// the polling. The ping carries the guild and the scopes as they stand, not
    /// the token: the token comes from the poll, the way CIBA's ping carries
    /// the `auth_req_id` and the tokens come from the token endpoint. A revoke
    /// carries what remains — empty when nothing does — so the device learns
    /// its token is dead without reading the diff.
    fn notify_grant_decided(&self, application_id: i64, guild_id: i64) {
        let pool = self.pool.clone();
        let transport = std::sync::Arc::clone(&self.transport);

        tokio::spawn(async move {
            send_grant_decided(&pool, &*transport, application_id, guild_id).await;
        });
    }

    /// Fire and forget for the same reason again: the parties have decided, and
    /// the application can read the contract whenever it likes — this is the
    /// ping that saves it from asking.
    fn notify_delegation_decided(
        &self,
        application_id: i64,
        target: vc_core::grant::Target,
        grant_id: i64,
        scopes: &[String],
    ) {
        let pool = self.pool.clone();
        let transport = std::sync::Arc::clone(&self.transport);
        let body = delegation_decided_body(target, grant_id, scopes);
        tokio::spawn(async move {
            send_to_application(&pool, &*transport, application_id, body).await;
        });
    }

    fn notify_contract_decided(&self, application_id: i64, contract_id: i64) {
        let pool = self.pool.clone();
        let transport = std::sync::Arc::clone(&self.transport);

        tokio::spawn(async move {
            send_contract_decided(&pool, &*transport, application_id, contract_id).await;
        });
    }
}

/// One delivery, if there is anywhere to deliver it.
async fn send_claim_update(
    pool: &sqlx::PgPool,
    transport: &dyn Transport,
    claimant_id: i32,
    events: &[Value],
) {
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

    send_to_application(pool, transport, application_id, claim_update_body(events)).await;
}

/// The event body for a grant decision: type 3, with the guild that answered
/// and the scopes it granted.
///
/// Type 1 is the PING the handshake sends, type 2 a claim update, and an
/// application must ignore types it does not know — which is what its
/// documentation says, and what makes adding this one safe for applications
/// written before it.
pub fn delegation_decided_body(
    target: vc_core::grant::Target,
    grant_id: i64,
    scopes: &[String],
) -> Value {
    serde_json::json!({"type": 3, "data": {"guild_id": target.guild().map(|id| id.to_string()), "discord_id": target.user().map(|id| id.to_string()),
            "grant_id": grant_id.to_string(), "scopes": scopes}})
}

pub fn grant_decided_body(guild_id: i64, scopes: &[String]) -> Value {
    serde_json::json!({
        "type": 3,
        "data": {
            "guild_id": guild_id.to_string(),
            "scopes": scopes,
        },
    })
}

/// One grant decision, if the guild's answer still stands as a grant.
///
/// The ping names the guild and the scopes *as they stand* — what an approval
/// wrote, or what a revoke left behind (empty when nothing does). A revoke
/// deletes the scopes, not the grant row, so there is always a row to read:
/// empty scopes are the revoke's own shape, not an absence. The one exception
/// is a grant nobody wrote or one explicitly deleted, in which case there is
/// nothing to say about and the ping stays unsent — the call sites revoke
/// scopes, never rows, so this is a guard rather than a branch anyone takes.
///
/// An application without a webhook polls instead, which is why this is a
/// ping and not the decision itself: the poll is what the token comes from.
async fn send_grant_decided(
    pool: &sqlx::PgPool,
    transport: &dyn Transport,
    application_id: i64,
    guild_id: i64,
) {
    let scopes = sqlx::query_scalar!(
        r#"SELECT COALESCE(array_agg(s.scope::text) FILTER (WHERE s.scope IS NOT NULL),
                           ARRAY[]::text[]) AS "scopes!"
             FROM grants g
             LEFT JOIN grant_scopes s ON s.grant_id = g.id
            WHERE g.application_id = $1 AND g.guild_id = $2
            GROUP BY g.id"#,
        application_id,
        guild_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let Some(scopes) = scopes else { return };

    send_to_application(
        pool,
        transport,
        application_id,
        grant_decided_body(guild_id, &scopes),
    )
    .await;
}

/// The event body for a contract decision: type 4, with the contract and its
/// parties as they stand.
///
/// Type 1 is the PING, 2 a claim update and 3 a grant decision, and an
/// application must ignore types it does not know — which is what makes adding
/// this one safe for applications written before it.
///
/// A party approving and the last party approving are **this one event with
/// different state**: the first arrives with `status: "pending"` and one more
/// approved party in the list, the second with `"active"`. That is how the claim
/// update carries its state, and it means an application that only cares about
/// "everyone is in" reads a field rather than joining two kinds of event.
pub fn contract_decided_body(contract: &vc_core::contract::Contract) -> Value {
    serde_json::json!({
        "type": 4,
        "data": {
            "contract": {
                "id": contract.id.to_string(),
                "unit": contract.unit,
                "guild_id": contract.guild_id.map(|guild| guild.to_string()),
                "status": contract.status,
                "receiver_discord_id": contract.receiver_discord_id.map(|id| id.to_string()),
                "expires_at": contract.expires_at.map(crate::routes::v2::claims::format_timestamp),
                "remaining": contract.remaining.to_string(),
            },
            "parties": contract
                .parties
                .iter()
                .map(|party| {
                    serde_json::json!({
                        "discord_id": party.discord_id.to_string(),
                        "amount": party.amount.to_string(),
                        "remaining": party.remaining.to_string(),
                        "status": party.status,
                    })
                })
                .collect::<Vec<Value>>(),
        },
    })
}

/// One contract decision, if there is still a contract to say anything about.
///
/// The read is here rather than at the call site for the grant path's reason:
/// what an application receives is what the contract *is* when the delivery is
/// made, not what it was when the decision was.
async fn send_contract_decided(
    pool: &sqlx::PgPool,
    transport: &dyn Transport,
    application_id: i64,
    contract_id: i64,
) {
    let Ok(found) = vc_core::contract::find(pool, contract_id).await else {
        return;
    };

    let Some(contract) = found else {
        return;
    };

    send_to_application(
        pool,
        transport,
        application_id,
        contract_decided_body(&contract),
    )
    .await;
}

/// One delivery to one application's webhook, if it named one — and if the
/// event is one it asked for.
///
/// Checked is sent and unchecked is not, and the column says which is which:
/// an application that unchecked claim updates does not get woken for one.
/// The check is here rather than in the callers because there are two of them
/// and one rule: a claim update is type 2 and a grant decision is type 3, and
/// the body says which before anything is signed.
async fn send_to_application(
    pool: &sqlx::PgPool,
    transport: &dyn Transport,
    application_id: i64,
    body: Value,
) {
    let kind = body.get("type").and_then(Value::as_i64).unwrap_or_default();

    let subscribed = sqlx::query_scalar!(
        "SELECT subscribed_events AS \"subscribed_events!\" FROM applications WHERE id = $1",
        application_id
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    if !subscribed.contains(&kind) {
        return;
    }

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

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default();

    let delivery = delivery(&body, &private_key, url, now);

    if let Err(error) = send(transport, &delivery).await {
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
    /// The answer never came: the proxy did not report one, or the webhook
    /// itself did not answer.
    Unreachable,
}

/// `_verify/3`'s two answers, decided.
///
/// Two requests, and each asks a different question. The one signed with the
/// application's real key must come back `200` carrying the PING back — an
/// application that answers webhooks at all. The one signed with a keypair
/// generated for the occasion must come back `401` — an application that
/// *checks* them. Which of the two goes first is [`verify`]'s to draw; here they
/// are named for what they are.
///
/// The fresh-key request is the whole point of doing this twice. An application
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
/// One of a handshake's two requests is signed with this rather than with the
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
/// Two requests, as the Elixir sends them. One is signed with a keypair generated
/// here and used for nothing else, because its whole purpose is to be a signature
/// the application should refuse — and the two go in an order drawn for the
/// occasion, because a fixed one is an order an application can answer without
/// verifying anything: `200` to whichever comes first and `401` to the second
/// passes a handshake whose real PING was first, and checking no signature at all.
pub async fn verify(
    transport: &dyn Transport,
    url: &str,
    private_key: &[u8; 32],
    at: i64,
) -> Handshake {
    let fresh = fresh_keypair();
    let real_first = getrandom::u32()
        .expect("the operating system's randomness")
        .is_multiple_of(2);

    let (first, second) = if real_first {
        (private_key, &fresh)
    } else {
        (&fresh, private_key)
    };

    let one = send_ping(transport, url, first, at).await;
    let two = send_ping(transport, url, second, at).await;

    // Which of the two was the real PING and which the one that must be refused,
    // said out of the order they arrived in.
    let (real, wrong) = if real_first { (one, two) } else { (two, one) };

    handshake(
        real.as_ref().map(|(status, body)| (*status, body)),
        wrong.as_ref().map(|(status, body)| (*status, body)),
    )
}

/// The whole handshake for one application's webhook: the transport a deployment
/// sends through, the moment it is signed for, and the two requests.
///
/// Three callers — registration, an edit, and the clock that re-checks what
/// registration once accepted — and the transport is none of their business. What
/// *is* their business travels back with the answer: whether a proxy was in the
/// way, which is what decides whose fault a silence is ([`crate::routes::
/// oauth2_clients`]'s `unverified` answers an application's `Failed` and this
/// service's `Unreachable`).
///
/// With no proxy configured the handshake goes straight at the webhook: a
/// development machine has no proxy, and a registration that names a `webhook_url`
/// has to be verifiable there for the application flow to be exercised at all.
pub async fn check_webhook(
    state: &crate::state::AppState,
    url: &str,
    private_key: &[u8; 32],
) -> (Handshake, bool) {
    let proxy = state.webhook_proxy();
    let direct = Direct::default();
    let transport: &dyn Transport = match proxy {
        Some(proxy) => proxy.as_ref(),
        None => &direct,
    };

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default();

    (
        verify(transport, url, private_key, at).await,
        proxy.is_some(),
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
    use vc_core::notification::Notifier;

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

    /// A webhook that answers the handshake's *order* rather than its requests:
    /// `200` and the PING back for whichever request arrives first, `401` for the
    /// one after it. It checks no signature at all — which is what a fixed order
    /// would let through.
    async fn order_answering_hook() -> String {
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let hook = move |_body: String| {
            let first = seen
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2);

            async move {
                if first {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(json!({ "type": PING })),
                    )
                } else {
                    (axum::http::StatusCode::UNAUTHORIZED, axum::Json(json!({})))
                }
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

    /// An application that verifies nothing cannot pass by learning the handshake's
    /// shape: the two requests go in an order drawn for the occasion, so answering
    /// `200` to the first and `401` to the second survives only the handshakes where
    /// the real PING happened to be the first.
    ///
    /// Thirty-two of them are what makes this a test rather than a coin toss: a
    /// fixed order would please this application every time, while a shuffled one
    /// leaves it a chance of one in four billion of passing them all.
    #[tokio::test]
    async fn an_application_that_answers_the_order_is_not_let_through() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let url = order_answering_hook().await;
        let direct = Direct::default();

        let mut passed = 0;

        for _ in 0..32 {
            if verify(&direct, &url, &signing.to_bytes(), 1_700_000_000).await == Handshake::Passed
            {
                passed += 1;
            }
        }

        assert!(
            passed < 32,
            "an application that verifies nothing passed every handshake"
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

    #[test]
    fn a_grant_decision_says_it_is_one() {
        let body = grant_decided_body(900_000_000_000_000_001, &["vc.issue".to_owned()]);

        assert_eq!(body["type"], 3);
        assert_eq!(body["data"]["guild_id"], "900000000000000001");
        assert_eq!(body["data"]["scopes"], json!(["vc.issue"]));
        assert_ne!(body["type"], PING, "which is the handshake's");
        assert_ne!(
            body["type"], 2,
            "which is the claim update's: a subscription tells them apart"
        );
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

    /// An application's webhook that keeps what it is sent, which is how a test
    /// reads a delivery back: the body an application would have verified.
    async fn recording_hook() -> (String, tokio::sync::mpsc::UnboundedReceiver<Value>) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

        let hook = move |body: String| {
            let sender = sender.clone();

            async move {
                sender
                    .send(serde_json::from_str(&body).expect("a json body"))
                    .expect("the test is still listening");

                axum::http::StatusCode::OK
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

        (format!("http://{address}/hook"), receiver)
    }

    /// The application a delivery is for: one that named this hook, subscribed
    /// to grant decisions by the column's default, holding a seed a delivery can
    /// be signed with — thirty-two bytes, because anything else is not an
    /// ed25519 seed and the delivery is dropped rather than signed with a guess.
    async fn application_at(pool: &sqlx::PgPool, url: &str) -> i64 {
        let signing = SigningKey::from_bytes(&[7u8; 32]);

        sqlx::query_scalar!(
            r#"INSERT INTO applications
                 (client_id, client_name, owner_discord_id, inserted_at, updated_at,
                  public_key, private_key, webhook_url)
               VALUES (gen_random_uuid(), 'an application', 900000000000000002, now(), now(),
                       $1, $2, $3)
               RETURNING id"#,
            signing.verifying_key().to_bytes().to_vec(),
            signing.to_bytes().to_vec(),
            url
        )
        .fetch_one(pool)
        .await
        .expect("an application")
    }

    /// The next delivery, waited for: a body that never arrives should fail the
    /// test rather than hang the suite.
    async fn next_delivery(
        delivered: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
        url: &str,
    ) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(10), delivered.recv())
            .await
            .unwrap_or_else(|_| panic!("nothing was delivered to {url}"))
            .expect("a body")
    }

    /// The ping carries the scopes *as they stand*, which is what makes a revoke
    /// worth pinging: the poll's answer carries none of them, so a device that
    /// reads this learns what its token can still do without asking for the
    /// difference. An approval and a taking-back are the same ping for the same
    /// reason.
    #[sqlx::test(migrations = "../vc-core/migrations")]
    async fn a_decision_is_pinged_with_the_scopes_as_they_stand(pool: sqlx::PgPool) {
        const GUILD: i64 = 900_000_000_000_000_001;

        let (url, mut delivered) = recording_hook().await;
        let application = application_at(&pool, &url).await;
        let client_id = sqlx::query_scalar!(
            r#"SELECT client_id::text AS "client_id!" FROM applications WHERE id = $1"#,
            application
        )
        .fetch_one(&pool)
        .await
        .expect("the client id");

        vc_core::grant::allow_in_guild(
            &pool,
            application,
            GUILD,
            &["vc.issue"],
            &[],
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("a grant");

        send_grant_decided(&pool, &Direct::default(), application, GUILD).await;

        assert_eq!(
            next_delivery(&mut delivered, &url).await,
            json!({
                "type": 3,
                "data": { "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] },
            }),
            "the approval's own scopes"
        );

        vc_core::grant::revoke_grant(&pool, &client_id, GUILD)
            .await
            .expect("a revoke");

        send_grant_decided(&pool, &Direct::default(), application, GUILD).await;

        assert_eq!(
            next_delivery(&mut delivered, &url).await,
            json!({
                "type": 3,
                "data": { "guild_id": GUILD.to_string(), "scopes": [] },
            }),
            "and none of them once the issuing scope is taken back"
        );
    }

    /// The notifier delivers over the transport it was given rather than through
    /// a proxy of its own: `Direct` here, which is what a machine with no proxy
    /// runs with — the same fallback the handshake makes, so a webhook that could
    /// be verified is also one that can be told about a decision.
    #[sqlx::test(migrations = "../vc-core/migrations")]
    async fn the_notifier_delivers_over_its_transport(pool: sqlx::PgPool) {
        const GUILD: i64 = 900_000_000_000_000_001;

        let (url, mut delivered) = recording_hook().await;
        let application = application_at(&pool, &url).await;

        vc_core::grant::allow_in_guild(
            &pool,
            application,
            GUILD,
            &["vc.issue"],
            &[],
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("a grant");

        WebhookNotifier::new(pool.clone(), std::sync::Arc::new(Direct::default()))
            .notify_grant_decided(application, GUILD);

        assert_eq!(
            next_delivery(&mut delivered, &url).await,
            json!({
                "type": 3,
                "data": { "guild_id": GUILD.to_string(), "scopes": ["vc.issue"] },
            })
        );
    }
}
