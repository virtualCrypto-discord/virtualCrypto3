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
use support::{
    FakeDiscord, Response, fake, get, insert_application, insert_user, mint, mint_app, state,
};
use tower::ServiceExt;

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const STRANGER_DISCORD_ID: i64 = 500_000_000_000_000_003;

const BOT_ID: i64 = 500_000_000_000_000_002;
/// Not any application's. The refusals that happen before the lookup do not read the
/// path, and this is how a test says it did not have to.
const A_CLIENT_ID: &str = "00000000-0000-0000-0000-000000000000";
/// Above 2^53, so a value that survives only as text.
const A_SNOWFLAKE: i64 = 900_000_000_000_000_001;
const A_SNOWFLAKE_AS_TEXT: &str = "900000000000000001";

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

async fn connect(pool: PgPool, token: &str, client_id: &str, body: Value) -> Response {
    connect_with(pool, fake(), token, client_id, body).await
}

async fn connect_with(
    pool: PgPool,
    discord: Arc<FakeDiscord>,
    token: &str,
    client_id: &str,
    body: Value,
) -> Response {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/applications/{client_id}/connect"))
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
        A_CLIENT_ID,
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
        A_CLIENT_ID,
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
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool.clone(),
        &token,
        &client_id,
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
    let theirs_client_id = client_id_of(&pool, theirs).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect(
        pool,
        &token,
        &theirs_client_id,
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
        A_CLIENT_ID,
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
        &client_id,
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
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let discord = Arc::new(FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT_ID, "something else entirely")],
    ));

    let response = connect_with(
        pool.clone(),
        discord,
        &token,
        &client_id,
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
        &client_id,
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
/// The integrations call is refused and the guild is refused too, which is Discord
/// saying the bot is not in that server. It is a 403 rather than a 404 because the
/// caller's own request was well formed: this is an answer about the world.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_the_bot_is_not_in_is_403_not_installed(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect_with(
        pool,
        Arc::new(FakeDiscord::with_statuses(403, 403)),
        &token,
        &client_id,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 403, "{:?}", response.body);
    assert_eq!(response.body["error"], "not_installed");
}

/// The integrations call is refused and the guild is readable, which is Discord saying
/// the bot is there but lacks Manage Server. The same refusal from Discord, a different
/// message, and the guild is named because the operator has to go and change a
/// permission in it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_without_manage_server_names_the_guild(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect_with(
        pool,
        Arc::new(FakeDiscord::with_statuses(403, 200)),
        &token,
        &client_id,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 403, "{:?}", response.body);
    assert_eq!(response.body["error"], "insufficient_permissions");

    let description = response.body["error_description"]
        .as_str()
        .expect("a description");

    assert!(description.contains("TestGuild"), "{description}");
}

/// A guild that does not exist is 404, and is not the same as a guild the service is not
/// in: there is nothing to install it into.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_guild_that_does_not_exist_is_404(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let response = connect_with(
        pool,
        Arc::new(FakeDiscord::with_statuses(404, 200)),
        &token,
        &client_id,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 404, "{:?}", response.body);
    assert_eq!(response.body["error"], "not_found");
}

/// The whole of it, the way the browser does it: read the list, take the id out of that
/// answer, post it back, and read the list again to see the application connected.
///
/// The id is taken from the response rather than written down here on purpose. If the
/// route insisted on this service's own numeric id, nothing the list answers could be
/// used to call it — which is the mistake that was in this route until the Elm list was
/// read, and the one this test is built to catch.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_id_the_list_hands_out_is_the_id_a_connect_takes(pool: PgPool) {
    insert_user(&pool, OWNER, OWNER_DISCORD_ID).await;
    let application = insert_application(&pool, OWNER_DISCORD_ID, "mine").await;
    let client_id = client_id_of(&pool, application).await;
    let token = mint(&pool, OWNER, &["oauth2.register"]).await;

    let discord = Arc::new(FakeDiscord::with_integrations(
        json!({ "name": "TestGuild" }),
        &[(BOT_ID, &client_id)],
    ));

    // What the application list shows, read the way the page reads it: the description
    // on the connect page has to name this string, so it is the one Discord is asked
    // about below.
    let listed = get(
        vc_api::router(state(pool.clone(), discord.clone())),
        "/oauth2/clients",
        Some(&token),
    )
    .await;

    assert_eq!(listed.status, 200, "{:?}", listed.body);
    let named = listed.body[0]["client_id"]
        .as_str()
        .expect("a client id in the list");

    assert_eq!(named, client_id);

    let response = connect_with(
        pool.clone(),
        discord,
        &token,
        named,
        json!({ "bot_id": BOT_ID.to_string(), "guild_id": A_SNOWFLAKE_AS_TEXT }),
    )
    .await;

    assert_eq!(response.status, 204, "{:?}", response.body);

    // And the state changed in the way the operator came to see: the application's own
    // account now carries the bot's Discord id, which is what connecting means.
    let after = get(
        vc_api::router(state(pool, fake())),
        "/oauth2/clients",
        Some(&token),
    )
    .await;

    assert_eq!(after.status, 200, "{:?}", after.body);
    assert_eq!(after.body[0]["discord_user_id"], BOT_ID.to_string());
}
