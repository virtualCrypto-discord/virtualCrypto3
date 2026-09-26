//! Contract tests for `GET /api/v2/users/@me`, checked against responses
//! captured from the Elixir implementation (see tests/golden/README.md).

mod support;

use sqlx::PgPool;
use support::{
    REFRESHED_REFRESH_TOKEN, REFRESHED_TOKEN, assert_matches_golden, discord_auth_row, fake, get,
    get_with_accept, golden, insert_discord_auth, insert_user, mint, revoke,
    set_discord_updated_at, state, utc_now,
};
use time::Duration;

const URI: &str = "/api/v2/users/@me";
const USER_ID: i32 = 1;
const DISCORD_ID: i64 = 100_000_000_000_000_001;
const SCOPES: [&str; 3] = ["oauth2.register", "vc.pay", "vc.claim"];

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER_ID, DISCORD_ID).await;
    insert_discord_auth(pool, DISCORD_ID, "stub-token").await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_matches_the_elixir_golden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_matches_golden(&response, &golden(include_str!("golden/v2_users_me.json")));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_without_authorization_is_400(pool: PgPool) {
    fixture(&pool).await;

    let response = get(vc_api::router(state(pool, fake())), URI, None).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_no_auth.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_with_a_malformed_token_is_401(pool: PgPool) {
    fixture(&pool).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some("not-a-jwt")).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_garbage_token.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_with_a_revoked_token_is_401(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;
    revoke(&pool, &token).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_revoked_token.json")),
    );
}

// --- content negotiation ---------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_without_an_accept_header_succeeds(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    let response =
        get_with_accept(vc_api::router(state(pool, fake())), URI, Some(&token), None).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_accept_absent.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_with_a_wildcard_accept_succeeds(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    let response = get_with_accept(
        vc_api::router(state(pool, fake())),
        URI,
        Some(&token),
        Some("*/*"),
    )
    .await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_accept_any.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_with_a_json_and_html_accept_succeeds(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    let response = get_with_accept(
        vc_api::router(state(pool, fake())),
        URI,
        Some(&token),
        Some("application/json, text/html"),
    )
    .await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_accept_json_and_html.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_with_an_html_only_accept_is_406(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    let response = get_with_accept(
        vc_api::router(state(pool, fake())),
        URI,
        Some(&token),
        Some("text/html"),
    )
    .await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_accept_html.json")),
    );
}

// --- Discord token refresh -------------------------------------------------

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_does_not_refresh_a_fresh_authorization(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;
    let discord = fake();

    let response = get(
        vc_api::router(state(pool, discord.clone())),
        URI,
        Some(&token),
    )
    .await;

    assert_matches_golden(&response, &golden(include_str!("golden/v2_users_me.json")));
    assert_eq!(
        discord.refresh_calls(),
        0,
        "a fresh authorization is not refreshed"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_refreshes_an_expiring_authorization(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER_ID, &SCOPES).await;

    // Move the stored authorization outside its seven-day lifetime.
    let stale = utc_now() - Duration::days(7);
    set_discord_updated_at(&pool, DISCORD_ID, stale).await;

    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let response = get(app.clone(), URI, Some(&token)).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_refresh.json")),
    );
    assert_eq!(
        discord.refresh_calls(),
        1,
        "an expiring authorization is refreshed once"
    );

    let (stored_token, stored_refresh, expires, updated_at) =
        discord_auth_row(&pool, DISCORD_ID).await;

    assert_eq!(stored_token.as_deref(), Some(REFRESHED_TOKEN));
    assert_eq!(stored_refresh.as_deref(), Some(REFRESHED_REFRESH_TOKEN));
    assert!(expires.unwrap() > utc_now() + Duration::minutes(15));
    // The Elixir code updates this row with update_all/2, which does not touch
    // updated_at, so the original value must survive the refresh.
    assert_eq!(updated_at, stale, "updated_at is not modified by a refresh");

    let second = get(app, URI, Some(&token)).await;
    assert_matches_golden(
        &second,
        &golden(include_str!("golden/v2_users_me_refresh.json")),
    );
    assert_eq!(
        discord.refresh_calls(),
        1,
        "the next request reuses the refreshed token despite the old updated_at"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn users_me_refreshes_before_the_stored_expiry_even_after_a_recent_login(pool: PgPool) {
    fixture(&pool).await;
    vc_core::user::insert_user(
        &pool,
        DISCORD_ID,
        "stub-token",
        Some("stub-refresh-token"),
        utc_now() + Duration::minutes(10),
    )
    .await
    .expect("record a login with a shorter token lifetime");
    let token = mint(&pool, USER_ID, &SCOPES).await;
    let discord = fake();

    let response = get(
        vc_api::router(state(pool.clone(), discord.clone())),
        URI,
        Some(&token),
    )
    .await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_refresh.json")),
    );
    assert_eq!(
        discord.refresh_calls(),
        1,
        "the stored expiry takes precedence"
    );
    let (stored_token, _, expires, _) = discord_auth_row(&pool, DISCORD_ID).await;
    assert_eq!(stored_token.as_deref(), Some(REFRESHED_TOKEN));
    assert!(expires.unwrap() > utc_now() + Duration::minutes(15));
}

/// `@me` is whoever holds the token, and an application holds one: its own account has no
/// Discord behind it, so what it reads about itself is its id and no profile — an answer rather
/// than the failure of a lookup there was nothing to make.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_account_without_a_discord_link_has_no_profile(pool: PgPool) {
    let application = support::insert_application(&pool, DISCORD_ID, "Shop").await;
    let account = support::account_of(&pool, application).await;
    let token = support::mint_app(&pool, account, &["vc.pay"]).await;

    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body,
        serde_json::json!({ "id": account.to_string(), "discord": null })
    );
}
