//! The `pay` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/pay_test.exs`.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{FakeDiscord, execute_from_guild, fake, get_amount, interaction, setup_money, state};

const COLOR_OK: i64 = 0x38EA42;

async fn pay(pool: PgPool, payload: Value) -> Value {
    let discord = fake();
    let response = interaction(vc_api::router(state(pool, discord.clone())), payload).await;
    assert_eq!(response.status, 202, "{}", response.body);
    assert_acknowledgement(&discord);
    discord.payment_finished().await;
    if let [body] = discord.webhooks().as_slice() {
        assert_eq!(body["flags"], 32768, "success is public");
        assert_eq!(discord.response_deletions(), 1);
        assert!(discord.response_edits().is_empty());
        body.clone()
    } else {
        assert!(discord.webhooks().is_empty(), "errors must stay private");
        assert_eq!(discord.response_deletions(), 0);
        let edits = discord.response_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["flags"], 32768);
        assert_eq!(edits[0].get("content"), Some(&Value::Null));
        assert_eq!(edits[0]["embeds"], json!([]));
        edits[0].clone()
    }
}

fn assert_acknowledgement(discord: &FakeDiscord) {
    assert_eq!(
        discord.callbacks(),
        vec![json!({
            "type": 4,
            "data": {"flags": 64, "content": "処理中…", "allowed_mentions": {"parse": []}}
        })]
    );
}

/// `InteractionsControllerTest.Pay.Helper.pay_data/1`.
fn pay_data(receiver: i64, amount: Value, unit: &str) -> Value {
    json!({
        "name": "pay",
        "options": [
            { "name": "user", "value": receiver.to_string() },
            { "name": "amount", "value": amount },
            { "name": "unit", "value": unit },
        ],
    })
}

/// `Helper.from_guild/2`.
fn from_guild(receiver: i64, amount: Value, unit: &str, sender: i64) -> Value {
    let mut payload = execute_from_guild(pay_data(receiver, amount, unit), sender);
    payload["application_id"] = json!("123");
    payload["token"] = json!("pay-token");
    payload
}

/// `Interactions.Pay.render/2` for `:error`, which every failure shares.
fn assert_error(response: &Value, content: &str) {
    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(content),
    );

    assert_eq!(response["flags"], json!(32768));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_unit_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;
    let amount = get_amount(&pool, money.user1, money.currency).await;

    let response = pay(
        pool,
        from_guild(money.user2, json!(amount), "void", money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨は存在しません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_positive_amount_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = pay(
        pool,
        from_guild(money.user2, json!(-1), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 不正な金額です。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_moves_the_amount_between_the_accounts(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = pay(
        pool.clone(),
        from_guild(money.user2, json!(20), &money.unit, money.user1),
    )
    .await;

    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**20** `{}`送金されました。",
            money.user1, money.user2, money.unit
        )),
    );

    assert_eq!(response["components"][0]["accent_color"], json!(COLOR_OK),);

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        sender_before - 20
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + 20
    );
}

/// The receiver has no account, so paying them creates one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_an_unknown_receiver_creates_their_account(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver = -1;

    let response = pay(
        pool.clone(),
        from_guild(receiver, json!(20), &money.unit, money.user1),
    )
    .await;

    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**20** `{}`送金されました。",
            money.user1, receiver, money.unit
        ))
    );

    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        sender_before - 20
    );
    assert_eq!(get_amount(&pool, receiver, money.currency).await, 20);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_more_than_the_balance_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = pay(
        pool,
        from_guild(money.user2, json!(1_000_000), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_the_whole_balance_empties_the_account(pool: PgPool) {
    let money = setup_money(&pool).await;
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = pay(
        pool.clone(),
        from_guild(money.user2, json!(sender_before), &money.unit, money.user1),
    )
    .await;

    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(format!(
            "<@{}>から<@{}>へ**{}** `{}`送金されました。",
            money.user1, money.user2, sender_before, money.unit
        ))
    );

    assert_eq!(get_amount(&pool, money.user1, money.currency).await, 0);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before + sender_before
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_one_more_than_the_balance_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;
    let amount = get_amount(&pool, money.user1, money.currency).await + 1;

    let response = pay(
        pool,
        from_guild(money.user2, json!(amount), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

/// An amount Discord sends as a string is parsed the same way. This one is past
/// `2^53`, which is exactly the sort of value a client would send as a string,
/// and it is far more than the sender holds.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_amount_sent_as_a_string_is_parsed(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = pay(
        pool,
        from_guild(
            money.user2,
            json!("9007199254740992"),
            &money.unit,
            money.user1,
        ),
    )
    .await;

    assert_error(&response, "エラー: 通貨が不足しています。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_wallet_lock_longer_than_three_seconds_is_acknowledged_and_paid_once(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM assets WHERE user_id = 1 AND currency_id = $1 FOR UPDATE")
        .bind(money.currency)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000001001");
    let started = std::time::Instant::now();
    let response = tokio::time::timeout(
        Duration::from_millis(2500),
        interaction(app.clone(), payload.clone()),
    )
    .await
    .expect("the initial response must not wait for the payment lock");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(response.status, 202);
    assert_acknowledgement(&discord);

    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert!(discord.webhooks().is_empty());
    assert!(discord.response_edits().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    // The receipt now records acceptance, even while the payment is still blocked.
    assert_eq!(interaction(app.clone(), payload.clone()).await.status, 202);
    blocker.rollback().await.unwrap();
    discord.payment_finished().await;
    assert_eq!(interaction(app, payload).await.status, 202);
    assert_eq!(discord.callbacks().len(), 1);
    assert_eq!(discord.webhooks().len(), 1);
    assert_eq!(discord.webhooks()[0]["flags"], 32768);
    assert_eq!(discord.response_deletions(), 1);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(records, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_rejected_or_timed_out_acknowledgement_never_pays(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    for (index, discord) in [
        FakeDiscord::with_callback_error(),
        FakeDiscord::with_callback_gate(gate.clone()),
    ]
    .into_iter()
    .enumerate()
    {
        let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
        payload["id"] = json!((900_000_000_000_002_000u64 + index as u64).to_string());
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            interaction(
                vc_api::router(state(pool.clone(), discord.clone())),
                payload.clone(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(response.status, 500);
        assert!(discord.callbacks().is_empty());
        assert!(discord.webhooks().is_empty());
        assert!(discord.response_edits().is_empty());
        assert_eq!(discord.response_deletions(), 0);
        let retry_discord = fake();
        let retry = interaction(
            vc_api::router(state(pool.clone(), retry_discord.clone())),
            payload,
        )
        .await;
        assert_eq!(retry.status, 500);
        assert!(retry_discord.callbacks().is_empty());
    }
    gate.add_permits(1);
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(records, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_failed_public_notification_keeps_the_committed_result_private(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let discord = FakeDiscord::with_followup(true, None);
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000003001");
    assert_eq!(interaction(app.clone(), payload.clone()).await.status, 202);
    discord.payment_finished().await;
    assert_acknowledgement(&discord);
    assert!(discord.webhooks().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    let edits = discord.response_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["components"][0]["accent_color"], COLOR_OK);
    assert!(
        edits[0]["components"][0]["components"][0]["content"]
            .as_str()
            .unwrap()
            .contains("送金されました。")
    );
    assert_eq!(edits[0]["flags"], 32768);
    assert_eq!(edits[0].get("content"), Some(&Value::Null));
    assert_eq!(interaction(app, payload).await.status, 202);
    assert_eq!(discord.callbacks().len(), 1);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_processing_message_remains_until_publication_succeeds(pool: PgPool) {
    let money = setup_money(&pool).await;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let discord = FakeDiscord::with_followup(false, Some(gate.clone()));
    let response = interaction(
        vc_api::router(state(pool, discord.clone())),
        from_guild(money.user2, json!(20), &money.unit, money.user1),
    )
    .await;
    assert_eq!(response.status, 202);
    tokio::time::timeout(Duration::from_secs(5), discord.followup_started())
        .await
        .unwrap();
    assert_acknowledgement(&discord);
    assert!(discord.webhooks().is_empty());
    assert!(discord.response_edits().is_empty());
    assert_eq!(discord.response_deletions(), 0);
    gate.add_permits(1);
    discord.payment_finished().await;
    assert_eq!(discord.webhooks().len(), 1);
    assert_eq!(discord.response_deletions(), 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failure_to_delete_processing_replaces_it_with_the_known_success(pool: PgPool) {
    let money = setup_money(&pool).await;
    let discord = FakeDiscord::with_response_delete_error();
    let response = interaction(
        vc_api::router(state(pool, discord.clone())),
        from_guild(money.user2, json!(20), &money.unit, money.user1),
    )
    .await;
    assert_eq!(response.status, 202);
    discord.payment_finished().await;
    assert_eq!(discord.webhooks().len(), 1);
    assert_eq!(discord.response_deletions(), 0);
    assert_eq!(discord.response_edits().len(), 1);
    assert_eq!(
        discord.response_edits()[0]["components"],
        discord.webhooks()[0]["components"]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_database_failure_updates_only_the_private_response(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    sqlx::query(
        "ALTER TABLE currency_payment_histories ADD CONSTRAINT injected_failure CHECK (amount < 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = pay(
        pool.clone(),
        from_guild(money.user2, json!(20), &money.unit, money.user1),
    )
    .await;
    assert_error(
        &response,
        "送金結果を確認できませんでした。送金履歴を確認してください。",
    );
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
}
