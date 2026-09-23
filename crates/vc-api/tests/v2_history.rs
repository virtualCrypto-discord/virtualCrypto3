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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_charges_credit_the_receivers_ledger_without_debiting_the_parties_twice(
    pool: PgPool,
) {
    let money = setup_money(&pool).await;
    support::insert_user(&pool, 9, OTHER).await;
    let application = insert_application(&pool, MONEY_USER1, "merchant service").await;
    let now = time::OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        &money.unit,
        &[
            vc_core::contract::NewParty {
                discord_id: MONEY_USER1,
                amount: 20,
            },
            vc_core::contract::NewParty {
                discord_id: MONEY_USER2,
                amount: 30,
            },
        ],
        Some(OTHER),
        None,
        now,
    )
    .await
    .unwrap();
    for user in [1, 2] {
        vc_core::contract::approve(&pool, contract, user, now)
            .await
            .unwrap();
    }
    vc_core::contract::pay(&pool, contract, application, OTHER, None, 35, now)
        .await
        .unwrap();
    let amount: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id=9 AND currency_id=$1")
            .bind(money.currency)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(amount, 35);

    let token = mint(&pool, 9, &[]).await;
    let app = router(pool.clone());
    // The merchant is not a contract party and cannot rely on its statement.
    assert_eq!(
        get(
            app.clone(),
            &format!("/api/v2/contracts/{contract}/payments"),
            Some(&token)
        )
        .await
        .status,
        404
    );
    let first = get(
        app.clone(),
        "/api/v2/users/@me/transactions?limit=1",
        Some(&token),
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body.as_array().unwrap().len(), 1);
    let next = next_from(&first);
    let second = get(
        app.clone(),
        &format!("/api/v2/users/@me/transactions?limit=1&next={next}"),
        Some(&token),
    )
    .await;
    assert_eq!(second.status, 200);
    assert_eq!(second.body.as_array().unwrap().len(), 1);
    assert_ne!(first.body[0]["id"], second.body[0]["id"]);
    let mut amounts = Vec::new();
    for response in [&first, &second] {
        let row = &response.body[0];
        assert_eq!(row["event"], "charge");
        assert_eq!(row["receiver_discord_id"], OTHER.to_string());
        assert_eq!(row["contract_client_name"], "merchant service");
        amounts.push(row["amount"].as_str().unwrap().parse::<i64>().unwrap());
    }
    amounts.sort();
    assert_eq!(amounts, [15, 20]);
    let filtered = get(
        app.clone(),
        &format!("/api/v2/users/@me/transactions?unit=n&related_discord_user_id={MONEY_USER1}"),
        Some(&token),
    )
    .await;
    assert_eq!(filtered.body.as_array().unwrap().len(), 1);
    assert_eq!(filtered.body[0]["amount"], "20");

    // The numbered reader used by Discord must count the same rows as the API.
    let page = vc_core::history::payments(&pool, 9, None, None, 1, 1)
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.next, Some(2));
    assert_eq!(page.rows.len(), 1);
    for user in [1, 2] {
        let token = mint(&pool, user, &[]).await;
        let payer = get(app.clone(), "/api/v2/users/@me/transactions", Some(&token)).await;
        assert_eq!(payer.body.as_array().unwrap().len(), 1);
        assert_eq!(payer.body[0]["event"], "lock");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_currency_replacement_does_not_expand_a_restricted_grants_history(pool: PgPool) {
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "history reader").await;
    let restricted = support::insert_grant_for(
        &pool,
        application,
        MONEY_GUILD,
        &["vc.issue"],
        &[money.currency],
    )
    .await;
    let app = router(pool.clone());
    let before = get(
        app.clone(),
        &format!("/api/v2/currencies/{}/issuances", money.currency),
        Some(&restricted),
    )
    .await;
    assert_eq!(before.status, 200);

    assert_eq!(
        vc_core::currency::delete(&pool, MONEY_GUILD, "delete n")
            .await
            .unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    vc_core::currency::create(&pool, MONEY_GUILD, "replacement", "new", MONEY_USER1, 1000)
        .await
        .unwrap();
    let replacement = vc_core::history::guild_currency(&pool, MONEY_GUILD)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(replacement, money.currency);
    vc_core::issue::issue(&pool, MONEY_GUILD, MONEY_USER2, Some(3))
        .await
        .unwrap();
    let uri = format!("/api/v2/currencies/{replacement}/issuances");

    let refused = get(app.clone(), &uri, Some(&restricted)).await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(refused.body["error"], "insufficient_scope");

    // A fresh grant for this currency, and an unrestricted grant, may read it.
    for resources in [vec![replacement], vec![]] {
        let token =
            support::insert_grant_for(&pool, application, MONEY_GUILD, &["vc.issue"], &resources)
                .await;
        let allowed = get(app.clone(), &uri, Some(&token)).await;
        assert_eq!(allowed.status, 200, "{}", allowed.body);
        assert_eq!(allowed.body.as_array().unwrap().len(), 1);
    }
}

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
