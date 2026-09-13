//! Contract tests for `POST /applications/{id}/connect`.
//!
//! These cover the request rather than the conversation with Discord. The happy path
//! needs a guild whose integrations contain this application's client id, which the
//! fake does not model yet — it answers an empty guild — so what is here is what can be
//! reached: the caller's kind, ownership, and the ids.
//!
//! The ids are the part worth testing. A Discord id is a snowflake around 10^18 and
//! JSON's number is a double that stops counting exactly at 2^53, so the ids arrive as
//! strings and are parsed here. `A_SNOWFLAKE` is above that boundary on purpose: a
//! `guild_id` of `"900000000000000001"` is the value that a client converting it with
//! `Number()` could not have sent, and the test that it gets past the parse is the test
//! that the endpoint is not doing that conversion somewhere of its own.
//!
//! The route had none of this until a review noticed `Number(guild_id)` in the
//! frontend, which is the same mistake in the same place one layer out.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{FakeDiscord, Response, fake, insert_user, mint, mint_app, state};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const STRANGER_DISCORD_ID: i64 = 500_000_000_000_000_003;

const BOT_ID: i64 = 500_000_000_000_000_002;
/// Above 2^53, so a value that survives only as text.
const A_SNOWFLAKE: i64 = 900_000_000_000_000_001;
const A_SNOWFLAKE_AS_TEXT: &str = "900000000000000001";

/// An application owned by `owner_discord_id`, and the account created for it.
///
/// Both rows, because the write at the end of a connect gives the *account* a Discord
/// id, and an application without one is an application that cannot be connected.
async fn insert_application(pool: &PgPool, owner_discord_id: i64, name: &str) -> i64 {
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

    sqlx::query!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now())",
        id
    )
    .execute(pool)
    .await
    .expect("the application's account");

    id
}

/// The application's `client_id`, which is the string the integration's description
/// must contain for a connect to be allowed.
async fn client_id_of(pool: &PgPool, application: i64) -> String {
    sqlx::query_scalar!(
        "SELECT client_id::text FROM applications WHERE id = $1",
        application
    )
    .fetch_one(pool)
    .await
    .expect("the row")
    // The column is nullable, but an application without a client id is not one.
    .expect("a client id")
}

async fn connect(pool: PgPool, token: &str, application_id: i64, body: Value) -> Response {
    connect_with(pool, fake(), token, application_id, body).await
}

async fn connect_with(
    pool: PgPool,
    discord: Arc<FakeDiscord>,
    token: &str,
    application_id: i64,
    body: Value,
) -> Response {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/applications/{application_id}/connect"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = vc_api::router(state(pool, discord))
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    // A connect that works answers 204 and nothing else, so an empty body is not an
    // error here.
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };

    Response {
        status,
        headers,
        body,
    }
}

/// A guild id that is not one answers 400, and does so before anything is read: an id
/// this service cannot hold is a malformed request, not a guild that does not exist.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_id_that_is_not_a_snowflake_is_400(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool,
        &token,
        1,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": "not-a-snowflake" }),
    )
    .await;

    assert_eq!(response.status, 400, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_request");
}

/// The same for the bot's id, which is also a snowflake and also arrives as text.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_id_that_is_not_a_snowflake_is_400(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool,
        &token,
        1,
        json!({ "bot_id": "not-a-snowflake", "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 400, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_request");
}

/// A snowflake above 2^53 gets past the parse, which is the point of carrying the ids
/// as text: had either side made a double of it, this would not be the same guild.
///
/// It stops at the fake's empty guild, and at the user lookup, which answers that there
/// is no such id.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_snowflake_above_2_53_is_read_exactly(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool.clone(),
        &token,
        1,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 404, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_bot");

    // And the number really is the one that was typed, rather than a rounded double.
    assert_eq!(
        A_SNOWFLAKE_AS_TEXT.parse::<i64>().expect("the id"),
        A_SNOWFLAKE
    );
}

/// An application belongs to its owner, and to nobody else. Anyone else is told there
/// is no such application rather than that it is not theirs, so that this endpoint
/// cannot be used to find out which applications exist.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_elses_application_is_404(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let theirs = insert_application(&pool, STRANGER_DISCORD_ID, "theirs").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool,
        &token,
        theirs,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 404, "{:?}", response.body);
    assert_eq!(response.body["error"], "not_found");
}

/// An application's own token is not a person's, and `PATCH /oauth2/clients/@me` says so
/// with `invalid_kind`. A connect is asked for by the person who owns the application,
/// so the same answer belongs here.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_app_token_is_401(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;

    // The token's subject is the account created for the application, not the
    // application: that account is what an app token asks about.
    let account = sqlx::query_scalar!(
        "SELECT id FROM users WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the application's account");

    let token = mint_app(&pool, account, &["oauth2.register"]).await;

    let response = connect(
        pool,
        &token,
        application,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 401, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_kind");
}

/// The whole of a connect: the integration is found by the bot's id, its description is
/// checked for the client id, and the bot's id is written onto the application's own
/// account. That account had no discord id, which is what makes it the application's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_described_with_the_client_id_is_bound(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let discord = Arc::new(FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT_ID, &client_id)],
    ));

    let response = connect_with(
        pool.clone(),
        discord,
        &token,
        application,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 204, "{:?}", response.body);

    let bound = sqlx::query_scalar!(
        "SELECT discord_id FROM users WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the account");

    assert_eq!(bound, Some(BOT_ID));
}

/// The bot is in the guild and the integration is there, but the description does not
/// name this application — so nothing is written. This is the check that keeps a bot
/// from being claimed by an application that merely knows its id.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_described_without_the_client_id_is_400(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let discord = Arc::new(FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT_ID, "something else entirely")],
    ));

    let response = connect_with(
        pool.clone(),
        discord,
        &token,
        application,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 400, "{:?}", response.body);
    assert_eq!(response.body["error"], "invalid_description");

    let bound = sqlx::query_scalar!(
        "SELECT discord_id FROM users WHERE application_id = $1",
        application
    )
    .fetch_one(&pool)
    .await
    .expect("the account");

    assert_eq!(bound, None, "nothing was written");
}

/// One bot belongs to one application, because `users_discord_id_index` is unique. The
/// second application is told so rather than being allowed to take it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_another_application_holds_is_409(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;

    let first = insert_application(&pool, OWNER_DISCORD_ID, "first").await;
    sqlx::query!(
        "UPDATE users SET discord_id = $1 WHERE application_id = $2",
        BOT_ID,
        first
    )
    .execute(&pool)
    .await
    .expect("the first application takes the bot");

    let second = insert_application(&pool, OWNER_DISCORD_ID, "second").await;
    let client_id = client_id_of(&pool, second).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let discord = Arc::new(FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT_ID, &client_id)],
    ));

    let response = connect_with(
        pool.clone(),
        discord,
        &token,
        second,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 409, "{:?}", response.body);
    assert_eq!(response.body["error"], "already_connected");

    // And the first application still has it, which is what the unique index means.
    let still = sqlx::query_scalar!(
        "SELECT discord_id FROM users WHERE application_id = $1",
        first
    )
    .fetch_one(&pool)
    .await
    .expect("the first account");

    assert_eq!(still, Some(BOT_ID));
}
