//! The two ledger endpoints: a person's payments and a currency's issuances.
//!
//! An addition rather than a port — the Elixir's `/api/v1|2/…/transactions` are writers and it
//! reads neither history table anywhere — so `docs/known-gaps.md` is the design, and this file
//! holds the API half of it to the same two rules the screens are held to: what may be seen
//! follows what may be done, and a page 2 is the same list as page 1.

mod support;

use axum::Router;
use sqlx::PgPool;
use support::{
    MONEY_GUILD, MONEY_USER1, MONEY_USER2, Response, fake, get, insert_application, mint,
    mint_guild_token, setup_money, state,
};

/// Somebody the money fixture does not have.
const OTHER: i64 = 100_000_000_000_000_003;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// One payment through the path `/pay` uses, so the ledger is written the way the service writes
/// it.
async fn pay(pool: &PgPool, sender: i64, receiver: i64, unit: &str, amount: i64) {
    vc_core::payment::pay_from_discord(pool, sender, receiver, unit, amount)
        .await
        .expect("a payment");
}

/// The `link` header, which is what a caller follows to the next page.
fn link(response: &Response) -> String {
    response
        .headers
        .get("link")
        .and_then(|value| value.to_str().ok())
        .expect("a next page")
        .to_owned()
}

/// Both directions on one list, and nobody else's rows in it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_transactions_list_answers_both_directions(pool: PgPool) {
    let money = setup_money(&pool).await;

    pay(&pool, MONEY_USER2, MONEY_USER1, &money.unit, 500).await;
    pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 300).await;

    let token = mint(&pool, 2, &[]).await;
    let listed = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions",
        Some(&token),
    )
    .await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);

    let rows = listed.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 2, "body: {}", listed.body);
    // Newest first: the 300 was sent second, and by this account's counterparty.
    assert_eq!(rows[0]["amount"], "300");
    assert_eq!(rows[0]["sender_discord_id"], MONEY_USER1.to_string());
    assert_eq!(rows[0]["receiver_discord_id"], MONEY_USER2.to_string());
    assert_eq!(rows[0]["unit"], money.unit);
    assert_eq!(rows[1]["amount"], "500");
    assert_eq!(rows[1]["sender_discord_id"], MONEY_USER2.to_string());

    // A filter narrows it, and the link that continues it keeps the filter: a page 2 without one
    // would be a different list.
    let filtered = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions?limit=1&unit=n",
        Some(&token),
    )
    .await;
    let rows = filtered.body.as_array().expect("a list").to_vec();

    assert_eq!(rows.len(), 1, "body: {}", filtered.body);
    assert!(link(&filtered).contains("unit=n"), "{}", link(&filtered));
}

/// A ledger is the account's own: a token for another account, and a delegated one, see neither
/// of its rows — the second by never reaching the list at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_transactions_list_is_the_callers_own(pool: PgPool) {
    let money = setup_money(&pool).await;
    pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 300).await;

    // The account that was not in it.
    support::insert_user(&pool, 9, OTHER).await;
    let stranger = mint(&pool, 9, &[]).await;
    let theirs = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions",
        Some(&stranger),
    )
    .await;

    assert_eq!(theirs.status, 200, "body: {}", theirs.body);
    assert_eq!(theirs.body.as_array().expect("a list").len(), 0);

    // A session token, which is an account's own, does see it.
    let own = mint(&pool, 2, &[]).await;
    let mine = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions",
        Some(&own),
    )
    .await;

    assert_eq!(mine.body.as_array().expect("a list").len(), 1);
}

/// The issuance ledger belongs to the guild that may spend from it: its own currency answers,
/// another one is answered the way a currency that is not there is, and a caller that is not a
/// guild token never reaches it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_issuances_list_is_the_guilds_own_currency(pool: PgPool) {
    let money = setup_money(&pool).await;

    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER1, Some(300))
        .await
        .expect("an issuance");
    vc_core::issue::issue(&pool, MONEY_GUILD, OTHER, Some(200))
        .await
        .expect("an issuance");

    let application = insert_application(&pool, MONEY_USER1, "a service").await;
    support::insert_grant(&pool, application, MONEY_GUILD, &["vc.issue"]).await;
    let guild = mint_guild_token(&pool, application, MONEY_GUILD).await;

    let listed = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{}/issuances", money.currency),
        Some(&guild),
    )
    .await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);

    let rows = listed.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 2, "body: {}", listed.body);
    assert_eq!(rows[0]["amount"], "200");
    assert_eq!(rows[0]["receiver_discord_id"], OTHER.to_string());

    // A currency that is not this token's guild's is not there, which is what a wrong number is.
    let elsewhere = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{}/issuances", money.currency2),
        Some(&guild),
    )
    .await;
    assert_eq!(elsewhere.status, 404, "body: {}", elsewhere.body);

    // And an account's own token is not a guild token, which the extractor says first.
    let account = mint(&pool, 2, &[]).await;
    let refused = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{}/issuances", money.currency),
        Some(&account),
    )
    .await;
    assert_eq!(refused.status, 401, "body: {}", refused.body);
}
