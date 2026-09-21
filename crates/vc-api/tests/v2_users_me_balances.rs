//! `GET /api/v2/users/@me/balances`, from the goldens captured for it.
//!
//! The two here are the endpoint's own refusals, and they need no holdings: a
//! request with no `Authorization` and one with a token that is not a token are
//! answered before anything is looked up. The other two goldens need a currency and
//! an asset each, which is a fixture rather than a value.

mod support;

use sqlx::PgPool;
use support::{assert_matches_golden, fake, get, golden, insert_user, mint, state};

const URI: &str = "/api/v2/users/@me/balances";

// The two accounts the goldens were captured with. The ids are the fixture's, not
// the API's: what the API shows is the discord id inside each currency.
const USER_ID: i32 = 1;
const DISCORD_ID: i64 = 500_000_000_000_000_001;
const OTHER_ID: i32 = 2;
const OTHER_DISCORD_ID: i64 = 500_000_000_000_000_002;
const SCOPES: &[&str] = &["vc.pay"];

/// A currency, with the name and the unit equal as they are in the Elixir's
/// fixture.
///
/// `inserted_at` and `updated_at` are written rather than left out: they are `NOT
/// NULL` with no default, and a missing one of those is not a compile error — sqlx
/// checks the columns a query mentions, not the ones it omits.
async fn insert_currency(pool: &PgPool, guild: i64, name: &str, pool_amount: i64) -> i64 {
    sqlx::query_scalar!(
        "INSERT INTO currencies (guild_id, name, unit, pool_amount, inserted_at, updated_at)
         VALUES ($1, $2, $2, $3, now(), now())
         RETURNING id",
        guild,
        name,
        pool_amount
    )
    .fetch_one(pool)
    .await
    .expect("a currency")
}

async fn insert_asset(pool: &PgPool, user: i64, currency: i64, amount: i64) {
    sqlx::query!(
        "INSERT INTO assets (user_id, currency_id, amount, inserted_at, updated_at)
         VALUES ($1, $2, $3, now(), now())",
        user,
        currency,
        amount
    )
    .execute(pool)
    .await
    .expect("an asset");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_authorization_is_400(pool: PgPool) {
    let response = get(vc_api::router(state(pool, fake())), URI, None).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_balances_no_auth.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn with_a_malformed_token_is_401(pool: PgPool) {
    let response = get(vc_api::router(state(pool, fake())), URI, Some("not-a-jwt")).await;

    assert_matches_golden(
        &response,
        &golden(include_str!(
            "golden/v2_users_me_balances_garbage_token.json"
        )),
    );
}

/// The first golden: one currency, one holding.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_single_holding_matches_the_elixir_golden(pool: PgPool) {
    insert_user(&pool, USER_ID, DISCORD_ID).await;
    let nyan = insert_currency(&pool, 900_000_000_000_000_001, "nyan", 500).await;
    insert_asset(&pool, i64::from(USER_ID), nyan, 199_500).await;

    let token = mint(&pool, USER_ID, SCOPES).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_balances.json")),
    );
}

/// The second: two currencies, and a holding in each of two accounts, which is what
/// settles the ordering — by the currency's unit, so `nyan` before `wan`.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn holdings_are_ordered_by_unit(pool: PgPool) {
    insert_user(&pool, USER_ID, DISCORD_ID).await;
    insert_user(&pool, OTHER_ID, OTHER_DISCORD_ID).await;

    let nyan = insert_currency(&pool, 900_000_000_000_000_001, "nyan", 500).await;
    let wan = insert_currency(&pool, 900_000_000_000_000_002, "wan", 1000).await;

    insert_asset(&pool, i64::from(USER_ID), nyan, 199_500).await;
    insert_asset(&pool, i64::from(OTHER_ID), nyan, 1_000).await;
    insert_asset(&pool, i64::from(OTHER_ID), wan, 200_000).await;

    let token = mint(&pool, OTHER_ID, SCOPES).await;
    let response = get(vc_api::router(state(pool, fake())), URI, Some(&token)).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_balances_user2.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn application_can_read_its_balance_without_discord_link(pool: PgPool) {
    let application = support::insert_application(&pool, 100000000000000001, "Shop").await;
    let account = support::account_of(&pool, application).await;
    support::insert_currency(&pool, 1, "test", "tst", 900000000000000001, 0).await;
    support::insert_asset(&pool, account, 1, 500).await;
    support::insert_user(&pool, account + 1, 100000000000000002).await;
    support::insert_asset(&pool, account + 1, 1, 999).await;
    let token = support::mint_app(&pool, account, &["vc.pay"]).await;
    let response = support::get(
        vc_api::router(support::state(pool, support::fake())),
        "/api/v2/users/@me/balances",
        Some(&token),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.body.as_array().unwrap().len(), 1);
    assert_eq!(response.body[0]["amount"], "500");
}
