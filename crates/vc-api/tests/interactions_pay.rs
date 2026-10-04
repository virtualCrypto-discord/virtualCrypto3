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
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        response.body["type"], 4,
        "the initial response is the final result"
    );
    assert_no_intermediate_reply(&discord);
    response.body["data"].clone()
}

fn assert_no_intermediate_reply(discord: &FakeDiscord) {
    assert!(
        discord.callbacks().is_empty(),
        "no processing/deferred callback"
    );
    assert!(discord.webhooks().is_empty(), "no extra result message");
    assert!(
        discord.response_edits().is_empty(),
        "no receipt to edit or delete"
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
    assert_eq!(response["components"][0]["accent_color"], 0xEA3875);
    assert_eq!(
        response["components"][0]["components"][0]["content"],
        json!(content),
    );

    assert_eq!(response["flags"], json!(32832), "errors are private");
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

    assert_error(&response, "**エラー**\nその通貨はありません。");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_positive_amount_is_reported(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = pay(
        pool,
        from_guild(money.user2, json!(-1), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "**エラー**\n枚数は1以上で指定してください。");
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
            "<@{}> から <@{}> に **20** `{}` を送金しました。",
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
            "<@{}> から <@{}> に **20** `{}` を送金しました。",
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
    let sender_before = get_amount(&pool, money.user1, money.currency).await;
    let receiver_before = get_amount(&pool, money.user2, money.currency).await;

    let response = pay(
        pool.clone(),
        from_guild(money.user2, json!(1_000_000), &money.unit, money.user1),
    )
    .await;

    assert_error(&response, "**エラー**\n残高が足りません。");
    assert_eq!(
        get_amount(&pool, money.user1, money.currency).await,
        sender_before
    );
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        receiver_before
    );
    assert_eq!(history_count(&pool).await, 0);
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
            "<@{}> から <@{}> に **{}** `{}` を送金しました。",
            money.user1,
            money.user2,
            vc_api::command::amount_text(sender_before),
            money.unit
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

    assert_error(&response, "**エラー**\n残高が足りません。");
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

    assert_error(&response, "**エラー**\n残高が足りません。");
}

const BUSY: &str =
    "**エラー**\n混み合っているため送金できませんでした。しばらくしてからもう一度お試しください。";
const UNCERTAIN: &str = "**エラー**\n送金できたか確認できませんでした。もう一度送る前に `/history pay` で確認してください。";

async fn history_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM currency_payment_histories")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_for_receipt(pool: &PgPool, id: &str, completed: bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM discord_interactions WHERE id = $1 AND (NOT $2 OR status IS NOT NULL))"
            ).bind(id).bind(completed).fetch_one(pool).await.unwrap();
            if found { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_wallet_lock_returns_a_private_refusal_and_rolls_back_everything(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM assets WHERE user_id = 1 AND currency_id = $1 FOR UPDATE")
        .bind(money.currency)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    // Resolving a new recipient happens before the locked balance write.
    let mut payload = from_guild(987654321, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000001001");
    let response = tokio::time::timeout(
        Duration::from_millis(2500),
        interaction(app.clone(), payload.clone()),
    )
    .await
    .expect("return a refusal before Discord's three-second deadline");
    assert_eq!(response.status, 200);
    assert_error(&response.body["data"], BUSY);
    assert_eq!(
        interaction(app.clone(), payload.clone()).await.body,
        response.body
    );
    blocker.rollback().await.unwrap();
    // Taking the same lock waits for the cancelled transaction to finish.
    sqlx::query("SELECT id FROM assets WHERE user_id = 1 AND currency_id = $1 FOR UPDATE")
        .bind(money.currency)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(get_amount(&pool, money.user1, money.currency).await, before);
    assert_eq!(history_count(&pool).await, 0);
    let created: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE discord_id = 987654321")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(created, 0, "recipient creation must roll back too");
    assert_eq!(interaction(app, payload).await.body, response.body);
    assert_no_intermediate_reply(&discord);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_blocked_receipt_does_not_start_a_late_payment(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user1, money.currency).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE discord_interactions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let response = tokio::time::timeout(
        Duration::from_millis(2500),
        pay(
            pool.clone(),
            from_guild(money.user2, json!(20), &money.unit, money.user1),
        ),
    )
    .await
    .unwrap();
    assert_error(&response, UNCERTAIN);
    blocker.rollback().await.unwrap();
    assert_eq!(get_amount(&pool, money.user1, money.currency).await, before);
    assert_eq!(history_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn success_is_one_public_initial_response_and_replay_does_not_pay_again(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    // The old callback path would fail; returning the result needs no REST call.
    let discord = FakeDiscord::with_callback_error();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000003001");
    let response = interaction(app.clone(), payload.clone()).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body["type"], 4);
    assert_eq!(response.body["data"]["flags"], 32768, "success is public");
    assert_eq!(
        response.body["data"]["allowed_mentions"],
        json!({"parse": []})
    );
    assert_eq!(
        response.body["data"]["components"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(interaction(app, payload).await.body, response.body);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
    assert_eq!(history_count(&pool).await, 1);
    assert_no_intermediate_reply(&discord);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_replay_never_starts_a_second_payment(pool: PgPool) {
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
    payload["id"] = json!("900000000000003002");
    let first = tokio::spawn(interaction(app.clone(), payload.clone()));
    wait_for_receipt(&pool, "900000000000003002", false).await;
    assert_eq!(interaction(app.clone(), payload.clone()).await.status, 409);
    blocker.rollback().await.unwrap();
    let response = first.await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body["data"]["flags"], 32768);
    assert_eq!(interaction(app, payload).await.body, response.body);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
    assert_eq!(history_count(&pool).await, 1);
    assert_no_intermediate_reply(&discord);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn disconnecting_the_request_preserves_the_result_and_never_repays(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let app = vc_api::router(state(pool.clone(), fake()));
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM assets WHERE user_id = 1 AND currency_id = $1 FOR UPDATE")
        .bind(money.currency)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000003003");
    let request = tokio::spawn(interaction(app.clone(), payload.clone()));
    wait_for_receipt(&pool, "900000000000003003", false).await;
    request.abort();
    assert!(matches!(request.await, Err(error) if error.is_cancelled()));
    blocker.rollback().await.unwrap();
    wait_for_receipt(&pool, "900000000000003003", true).await;
    let replay = interaction(app, payload).await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body["data"]["flags"], 32768);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
    assert_eq!(history_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn paying_in_a_dm_returns_the_result_directly(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["user"] = payload["member"]["user"].clone();
    payload.as_object_mut().unwrap().remove("member");
    payload.as_object_mut().unwrap().remove("guild_id");
    let response = pay(pool.clone(), payload).await;
    assert_eq!(response["flags"], 32768);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_database_failure_returns_only_a_private_error_and_rolls_back(pool: PgPool) {
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
        "**エラー**\n送金できませんでした。しばらくしてからもう一度お試しください。",
    );
    assert_eq!(get_amount(&pool, money.user2, money.currency).await, before);
    assert_eq!(history_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_commit_timeout_is_private_and_never_claims_the_payment_was_cancelled(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let mut blocker = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(73003)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION delay_payment_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(73003); RETURN NEW; END $$;
        CREATE CONSTRAINT TRIGGER delay_payment_commit AFTER INSERT ON currency_payment_histories
        DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION delay_payment_commit();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = vc_api::router(state(pool.clone(), fake()));
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000003004");
    let response = tokio::time::timeout(
        Duration::from_millis(2800),
        interaction(app.clone(), payload.clone()),
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_error(&response.body["data"], UNCERTAIN);
    assert_eq!(
        interaction(app.clone(), payload.clone()).await.body,
        response.body
    );
    sqlx::query("SELECT pg_advisory_unlock(73003)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while get_amount(&pool, money.user2, money.currency).await != before + 20 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(interaction(app, payload).await.body, response.body);
    assert_eq!(history_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_slow_receipt_save_still_returns_the_committed_result(pool: PgPool) {
    let money = setup_money(&pool).await;
    let before = get_amount(&pool, money.user2, money.currency).await;
    let mut blocker = pool.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock(73004)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION delay_receipt_save() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(73004); RETURN NEW; END $$;
        CREATE TRIGGER delay_receipt_save BEFORE UPDATE ON discord_interactions
        FOR EACH ROW EXECUTE FUNCTION delay_receipt_save();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = vc_api::router(state(pool.clone(), fake()));
    let mut payload = from_guild(money.user2, json!(20), &money.unit, money.user1);
    payload["id"] = json!("900000000000003005");
    let response = tokio::time::timeout(
        Duration::from_millis(2800),
        interaction(app.clone(), payload.clone()),
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body["data"]["flags"], 32768);
    assert_eq!(
        get_amount(&pool, money.user2, money.currency).await,
        before + 20
    );
    sqlx::query("SELECT pg_advisory_unlock(73004)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    wait_for_receipt(&pool, "900000000000003005", true).await;
    assert_eq!(interaction(app, payload).await.body, response.body);
    assert_eq!(history_count(&pool).await, 1);
}
