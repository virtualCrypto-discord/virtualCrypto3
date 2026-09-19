//! Contract decisions, delivered to the application that wrote the contract.
//!
//! The whole path, for the reason `tests/notification.rs` gives about claims:
//! what an application receives is a signed body over HTTP, and the two halves
//! that decide it — the shape of the event and whether the application's
//! subscription wants it — are worth pinning against a webhook that is really
//! there.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Recorded, account_of, fake, insert_application, insert_asset, insert_currency, insert_user,
    mint, mint_app, state_with_notifier,
};
use tower::ServiceExt;
use vc_core::notification::Notifier;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const PARTY: i32 = 2;
const PARTY_DISCORD_ID: i64 = 100_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;
/// A seed, because a delivery is signed with the application's own key and one
/// that is not thirty-two bytes is dropped rather than signed with a guess.
const SEED: [u8; 32] = [7u8; 32];

/// An application's webhook that keeps what it is sent.
async fn hook() -> (String, tokio::sync::mpsc::UnboundedReceiver<Value>) {
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

/// The next delivery, waited for: one that never arrives should fail the test
/// rather than hang the suite.
async fn next_delivery(
    delivered: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    url: &str,
) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), delivered.recv())
        .await
        .unwrap_or_else(|_| panic!("nothing was delivered to {url}"))
        .expect("a body")
}

/// An application whose webhook is `url` and which wants `subscribed`.
///
/// The users a test needs are made before this, because the account an
/// application is given is a `users` row too and takes the next id.
async fn application_at(pool: &PgPool, url: &str, subscribed: &[i64]) -> i64 {
    let application = insert_application(pool, OWNER_DISCORD_ID, "an application").await;
    let signing = ed25519_dalek::SigningKey::from_bytes(&SEED);

    sqlx::query!(
        "UPDATE applications
            SET webhook_url = $2, private_key = $3, public_key = $4, subscribed_events = $5
          WHERE id = $1",
        application,
        url,
        signing.to_bytes().to_vec(),
        signing.verifying_key().to_bytes().to_vec(),
        subscribed
    )
    .execute(pool)
    .await
    .expect("the webhook");

    application
}

async fn request(
    app: axum::Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Value,
    expected: u16,
) -> Value {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {token}"));

    if !body.is_null() {
        builder = builder.header("content-type", "application/json");
    }

    let request = builder
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };

    assert_eq!(status, expected, "body: {parsed}");

    parsed
}

/// A party approving is a decision the application hears about, and what it
/// receives is the contract as it stands — including the amount that just became
/// spendable.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_contract_decision_is_delivered(pool: PgPool) {
    let (url, mut delivered) = hook().await;

    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(&pool, PARTY, PARTY_DISCORD_ID).await;
    insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(&pool, PARTY, 1, 1_000).await;

    let application = application_at(&pool, &url, &[4]).await;

    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["vc.contract"]).await;
    let party = mint(&pool, PARTY, &[]).await;

    let notifier: Arc<dyn Notifier> = Arc::new(vc_api::notification::WebhookNotifier::new(
        pool.clone(),
        Arc::new(vc_api::notification::Direct::default()),
    ));
    let router = || vc_api::router(state_with_notifier(pool.clone(), fake(), notifier.clone()));

    let created = request(
        router(),
        "POST",
        "/api/v2/contracts",
        &token,
        json!({
            "unit": "nyan",
            "parties": [{ "discord_id": PARTY_DISCORD_ID.to_string(), "amount": "100" }],
        }),
        201,
    )
    .await;

    let id = created["id"].as_str().expect("an id").to_owned();

    request(
        router(),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        &party,
        Value::Null,
        200,
    )
    .await;

    assert_eq!(
        next_delivery(&mut delivered, &url).await,
        json!({
            "type": 4,
            "data": {
                "contract": {
                    "id": id,
                    "unit": "nyan",
                    "guild_id": GUILD.to_string(),
                    "status": "active",
                    "receiver_discord_id": null,
                    "expires_at": null,
                    "remaining": "100",
                },
                "parties": [{
                    "discord_id": PARTY_DISCORD_ID.to_string(),
                    "amount": "100",
                    "remaining": "100",
                    "status": "approved",
                }],
            },
        })
    );
}

/// A subscription is what it says: an application that asked for claims and
/// grants is not woken for a contract.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_that_did_not_ask_hears_nothing(pool: PgPool) {
    let (url, mut delivered) = hook().await;

    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(&pool, PARTY, PARTY_DISCORD_ID).await;
    insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(&pool, PARTY, 1, 1_000).await;

    let application = application_at(&pool, &url, &[2, 3]).await;

    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["vc.contract"]).await;
    let party = mint(&pool, PARTY, &[]).await;

    let notifier: Arc<dyn Notifier> = Arc::new(vc_api::notification::WebhookNotifier::new(
        pool.clone(),
        Arc::new(vc_api::notification::Direct::default()),
    ));
    let router = || vc_api::router(state_with_notifier(pool.clone(), fake(), notifier.clone()));

    let created = request(
        router(),
        "POST",
        "/api/v2/contracts",
        &token,
        json!({
            "unit": "nyan",
            "parties": [{ "discord_id": PARTY_DISCORD_ID.to_string(), "amount": "100" }],
        }),
        201,
    )
    .await;

    request(
        router(),
        "POST",
        &format!(
            "/api/v2/contracts/{}/approval",
            created["id"].as_str().expect("an id")
        ),
        &party,
        Value::Null,
        200,
    )
    .await;

    // A negative assertion costs a wait, and this is the only way to make one:
    // the delivery would be made by a spawned task, so "nothing yet" has to be
    // given a while to be worth saying.
    let arrived = tokio::time::timeout(std::time::Duration::from_secs(1), delivered.recv()).await;

    assert!(
        arrived.is_err(),
        "an application that did not subscribe to contracts was told about one: {arrived:?}"
    );
}

/// Only a decision is delivered: approving twice tells the application once, and
/// taking the delegation back tells it again.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn only_a_decision_is_delivered(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(&pool, PARTY, PARTY_DISCORD_ID).await;
    insert_currency(&pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(&pool, PARTY, 1, 1_000).await;

    let application = insert_application(&pool, OWNER_DISCORD_ID, "an application").await;

    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["vc.contract"]).await;
    let party = mint(&pool, PARTY, &[]).await;
    let recorded = Arc::new(Recorded::default());

    let router = || vc_api::router(state_with_notifier(pool.clone(), fake(), recorded.clone()));

    let created = request(
        router(),
        "POST",
        "/api/v2/contracts",
        &token,
        json!({
            "unit": "nyan",
            "parties": [{ "discord_id": PARTY_DISCORD_ID.to_string(), "amount": "100" }],
        }),
        201,
    )
    .await;

    let id: i64 = created["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a number");

    request(
        router(),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        &party,
        Value::Null,
        200,
    )
    .await;

    assert_eq!(
        recorded.contract_decisions(),
        [(application, id)],
        "the approval"
    );

    request(
        router(),
        "POST",
        &format!("/api/v2/contracts/{id}/approval"),
        &party,
        Value::Null,
        200,
    )
    .await;

    assert_eq!(
        recorded.contract_decisions(),
        [(application, id)],
        "and not the same approval twice"
    );

    request(
        router(),
        "DELETE",
        &format!("/api/v2/contracts/{id}/approval"),
        &party,
        Value::Null,
        200,
    )
    .await;

    assert_eq!(
        recorded.contract_decisions(),
        [(application, id), (application, id)],
        "and then the taking-back"
    );
}
