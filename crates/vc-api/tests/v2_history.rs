//! The two ledger endpoints: a person's own movements and a currency's issuances.
//!
//! An addition rather than a port — the Elixir's `/api/v1|2/…/transactions` are writers and it
//! reads neither history table anywhere — so `docs/known-gaps.md` is the design, and this file
//! holds the API half of it to the same three rules the screens are held to: what may be seen
//! follows what may be done, a page 2 is the same list as page 1, and the person's list is both
//! ledgers at once — the payments and what the pool issued to them, merged newest first, which
//! here is `time`, then the ledger, then the id.

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

/// The cursor a full page offers, as the link header spells it.
fn next_from(response: &Response) -> String {
    let link = link(response);
    let start = link.find("next=").expect("a cursor") + "next=".len();
    let rest = &link[start..];
    let end = rest.find(['&', '>']).unwrap_or(rest.len());

    rest[..end].to_owned()
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

/// An issuance to the caller is money arriving in their wallet, so it is in their list: the same
/// list as the payments, `event` saying `issue`, and no sender at all — the pool paid, and the
/// pool has no Discord id.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_issuance_to_the_caller_is_in_their_list(pool: PgPool) {
    let money = setup_money(&pool).await;

    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(300))
        .await
        .expect("an issuance");

    let token = mint(&pool, 2, &[]).await;
    let listed = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions",
        Some(&token),
    )
    .await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);

    let rows = listed.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 1, "body: {}", listed.body);

    assert_eq!(rows[0]["ledger"], "issuance");
    assert_eq!(rows[0]["event"], "issue");
    assert_eq!(rows[0]["amount"], "300");
    assert_eq!(rows[0]["unit"], money.unit);
    assert_eq!(rows[0]["receiver_discord_id"], MONEY_USER2.to_string());
    assert_eq!(
        rows[0]["sender_discord_id"],
        serde_json::Value::Null,
        "no sender: the pool paid"
    );
    assert_eq!(rows[0]["contract_client_name"], serde_json::Value::Null);

    // Somebody else was not issued anything, so their list is empty.
    let other = mint(&pool, 1, &[]).await;
    let theirs = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions",
        Some(&other),
    )
    .await;
    assert_eq!(
        theirs.body.as_array().expect("a list").len(),
        0,
        "body: {}",
        theirs.body
    );
}

/// The merged order and the cursor that resumes in it: `time` newest first, the ledger breaking a
/// second's tie — an issuance before a payment — and the id breaking what is left. The cursor is
/// the last row's own place in that order, so page 2 is the rest of *this* list.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_merged_list_is_ordered_and_its_cursor_resumes_it(pool: PgPool) {
    setup_money(&pool).await;

    // Written with times of their own: two writers in one test usually share a second and
    // sometimes do not, and the tie is the thing under test here.
    let same_second = time::macros::datetime!(2026-01-01 00:00:02);
    let a_second_earlier = time::macros::datetime!(2026-01-01 00:00:01);

    // A payment and an issuance in the same second, and an older payment.
    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         SELECT 10, sender.id, receiver.id, 1, $3, $3, $3
           FROM users sender, users receiver
          WHERE sender.discord_id = $1 AND receiver.discord_id = $2",
        MONEY_USER1,
        MONEY_USER2,
        same_second
    )
    .execute(&pool)
    .await
    .expect("a payment");

    sqlx::query!(
        "INSERT INTO currency_given_histories
             (amount, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         SELECT 300, receiver.id, 1, $2, $2, $2 FROM users receiver WHERE receiver.discord_id = $1",
        MONEY_USER2,
        same_second
    )
    .execute(&pool)
    .await
    .expect("an issuance");

    sqlx::query!(
        "INSERT INTO currency_payment_histories
             (amount, sender_id, receiver_id, currency_id, \"time\", inserted_at, updated_at)
         SELECT 20, sender.id, receiver.id, 1, $3, $3, $3
           FROM users sender, users receiver
          WHERE sender.discord_id = $1 AND receiver.discord_id = $2",
        MONEY_USER1,
        MONEY_USER2,
        a_second_earlier
    )
    .execute(&pool)
    .await
    .expect("a payment");

    let token = mint(&pool, 2, &[]).await;
    let first = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions?limit=2",
        Some(&token),
    )
    .await;

    let rows = first.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 2, "body: {}", first.body);
    assert_eq!(
        rows[0]["ledger"], "issuance",
        "the same second, and the pool's arrival reads first: {}",
        first.body
    );
    assert_eq!(rows[0]["amount"], "300");
    assert_eq!(rows[1]["ledger"], "payment");
    assert_eq!(rows[1]["amount"], "10");

    // The cursor is the last row's own place, spelled as `docs/api.rs` documents it.
    assert_eq!(next_from(&first), "2026-01-01T00:00:02Z:0:1");

    // `next` resumes after it: the rest of the list, newest first.
    let rest = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions?limit=2&next=2026-01-01T00:00:02Z:0:1",
        Some(&token),
    )
    .await;

    let rows = rest.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 1, "body: {}", rest.body);
    assert_eq!(rows[0]["amount"], "20");

    // And `on_next` includes the row it names, which is the same list one place back.
    let from_here = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions?limit=2&on_next=2026-01-01T00:00:02Z:0:1",
        Some(&token),
    )
    .await;

    let rows = from_here.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 2, "body: {}", from_here.body);
    assert_eq!(rows[0]["amount"], "10");
    assert_eq!(rows[1]["amount"], "20");

    // A cursor that cannot be read is the complaint a non-numeric id gets.
    let refused = get(
        router(pool.clone()),
        "/api/v2/users/@me/transactions?next=nonsense",
        Some(&token),
    )
    .await;
    assert_eq!(refused.status, 400, "body: {}", refused.body);

    // The counterparty filter asks about a person on the other side, and an issuance has none.
    let narrowed = get(
        router(pool.clone()),
        &format!("/api/v2/users/@me/transactions?related_discord_user_id={MONEY_USER1}"),
        Some(&token),
    )
    .await;

    let rows = narrowed.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 2, "body: {}", narrowed.body);
    assert!(
        rows.iter().all(|row| row["ledger"] == "payment"),
        "{}",
        narrowed.body
    );
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
