//! `GET /oauth2/clients/@me`: the application the token is for.
//!
//! This is RFC 7592's read of the client the token identifies, and it is the URI that
//! `POST /oauth2/clients` answers as `registration_client_uri`. So it takes the
//! **application** token that registration issued, and the test that matters most is
//! the one below that does exactly that: register, then read itself at the address it
//! was handed.
//!
//! The list is not here. It is `oauth2_clients_mine.rs`, on `/oauth2/clients`. Both
//! answered `/@me` until this was corrected — the list had the path, and the read had
//! been promised it by the registration it comes from.
//!
//! The last test registers, because the address this path is, is the one registration
//! answers — so it is the registration's own answer that is taken apart and followed.
//!
//! It was first written off as untestable, on the claim that the test support had no
//! fixture for a stored Discord authorization. It does: `support::insert_discord_auth`,
//! and `set_discord_updated_at` and `discord_auth_row` beside it. That claim was made
//! without looking, in the same way as the ones this file's neighbours were written to
//! correct.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{fake, get, insert_discord_auth, insert_user, mint, mint_app, state};
use tower::ServiceExt;

const URI: &str = "/oauth2/clients/@me";
const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

/// An application owned by `owner_discord_id`, and the id of the account created for
/// it. The account is what an application token's subject is.
async fn insert_application(pool: &PgPool, owner_discord_id: i64, name: &str) -> (i64, i32) {
    let id = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (client_id, client_name, owner_discord_id, inserted_at, updated_at,
              public_key, private_key)
           VALUES (gen_random_uuid(), $1, $2, now(), now(), '\x00'::bytea, '\x00'::bytea)
           RETURNING id"#,
        name,
        owner_discord_id
    )
    .fetch_one(pool)
    .await
    .expect("an application");

    let account = sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now()) RETURNING id",
        id
    )
    .fetch_one(pool)
    .await
    .expect("the application's account");

    (id, account)
}

/// A client reads itself with the token it was given, which is the whole of what RFC
/// 7592's read is for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_token_reads_its_own_application(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = sqlx::query_scalar!(
        "SELECT client_id::text FROM applications WHERE id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the row")
    .expect("a client id");
    let token = mint_app(&pool, account, &["oauth2.register"]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "{:?}", response.body);
    assert_eq!(response.body["client_id"], client_id);
    assert_eq!(response.body["client_name"], "mine");
}

/// A person is not a client reading itself, and this path is the client's. The same
/// answer `PATCH` on this path gives, since they are the two halves of one call.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_token_is_refused(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 401, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_kind");
}

/// An application token without the registration scope is refused the way `PATCH` on
/// this path refuses it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_the_scope_is_403(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let (_, account) = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint_app(&pool, account, &[]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 403, "{:?}", response.body);
    assert_eq!(response.body["error"], "insufficient_scope");
}

/// The whole reason the read was missing: registration answers an address, and a client
/// that goes to it has to be answered. This registers, takes the URI and the token out
/// of that answer, and reads the application back with them.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_address_registration_hands_out_is_readable(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_discord_auth(&pool, OWNER_DISCORD_ID, "a-discord-token").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/clients")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "client_name": "one",
                "redirect_uris": ["https://example.test/callback"],
            }))
            .expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let registered: Value = serde_json::from_slice(&bytes).expect("json body");

    assert_eq!(status, 201, "{registered:?}");

    let uri = registered["registration_client_uri"]
        .as_str()
        .expect("a registration_client_uri");

    assert!(
        uri.ends_with("/oauth2/clients/@me"),
        "the address it hands out is this one: {uri}"
    );

    let registration_token = registered["registration_access_token"]
        .as_str()
        .expect("a registration_access_token");

    let read = get(
        vc_api::router(state(pool, fake())),
        "/oauth2/clients/@me",
        Some(registration_token),
    )
    .await;

    assert_eq!(read.status, 200, "{:?}", read.body);
    assert_eq!(read.body["client_id"], registered["client_id"]);
    assert_eq!(
        read.body["subscribed_events"],
        serde_json::json!([2, 3]),
        "registered without naming any: everything, spelled out"
    );
}

/// The response types a registration names are what the row gets.
///
/// They were a literal empty array until now: `validate_response_types/1` ran and its answer was
/// thrown away, which is the one thing in this endpoint that was ported from the Elixir with the
/// knowledge that it was wrong. Nothing downstream reads the field, so the row is where to look.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn registration_stores_the_response_types_it_validated(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_discord_auth(&pool, OWNER_DISCORD_ID, "a-discord-token").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/clients")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "client_name": "one",
                "redirect_uris": ["https://example.test/callback"],
                "response_types": ["code"],
            }))
            .expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 201);

    let stored = sqlx::query_scalar!(
        r#"SELECT response_types::text[] AS "response_types!" FROM applications"#
    )
    .fetch_one(&pool)
    .await
    .expect("the application");

    assert_eq!(stored, vec!["code".to_owned()]);
}

/// The events a registration names are what the row gets, and what the read
/// answers back: the subscription is the application's, from the start.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn registration_stores_the_events_it_named(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_discord_auth(&pool, OWNER_DISCORD_ID, "a-discord-token").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/clients")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "client_name": "one",
                "redirect_uris": ["https://example.test/callback"],
                "subscribed_events": [3],
            }))
            .expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 201);

    let stored = sqlx::query_scalar!(
        r#"SELECT subscribed_events AS "subscribed_events!" FROM applications"#
    )
    .fetch_one(&pool)
    .await
    .expect("the application");

    assert_eq!(stored, [3]);
}

/// An event type nobody sends is refused at the door, the way an unknown scope
/// is: a subscription to nothing the service emits is a subscription to
/// nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn registration_refuses_an_unknown_event(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_discord_auth(&pool, OWNER_DISCORD_ID, "a-discord-token").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/clients")
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "client_name": "one",
                "redirect_uris": ["https://example.test/callback"],
                "subscribed_events": [9],
            }))
            .expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .expect("router response");

    assert_eq!(response.status().as_u16(), 400);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json body");

    assert_eq!(body["error"], "invalid_client_metadata");
    assert_eq!(
        body["error_description"],
        "subscribed_events_must_be_known_event_types"
    );
}
