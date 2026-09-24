//! `vc_core::user::bind_bot`: the write the connect flow exists to make.
//!
//! Ordinary accounts created by payments or claims merge into the application;
//! a bot that already speaks for another application cannot be claimed again.

mod support;

use sqlx::PgPool;
use vc_core::user::{BindError, bind_bot};

const BOT: i64 = 500_000_000_000_000_001;

/// An application's account, which is what registration creates: a `users` row with
/// an application and no Discord id of its own.
///
/// The application row has to exist first — `users.application_id` is a foreign key —
/// which is why this is two inserts.
async fn insert_application_account(pool: &PgPool, name: &str) -> i32 {
    let application = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (client_id, client_name, inserted_at, updated_at, public_key, private_key)
           VALUES (gen_random_uuid(), $1, now(), now(), '\x00'::bytea, '\x00'::bytea)
           RETURNING id"#,
        name
    )
    .fetch_one(pool)
    .await
    .expect("an application");

    sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now())
         RETURNING id",
        application
    )
    .fetch_one(pool)
    .await
    .expect("the application's account")
}

async fn discord_id_of(pool: &PgPool, user_id: i32) -> Option<i64> {
    sqlx::query_scalar!("SELECT discord_id FROM users WHERE id = $1", user_id)
        .fetch_one(pool)
        .await
        .expect("the account")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_gives_the_account_the_bots_id(pool: PgPool) {
    let account = insert_application_account(&pool, "one").await;

    assert!(matches!(bind_bot(&pool, account, BOT).await, Ok(())));
    assert_eq!(discord_id_of(&pool, account).await, Some(BOT));
}

/// The branch with a message of its own: 「すでにそのBotは別のApplicationに紐付け
/// られています。」 The database decides it, not a check beforehand, which is why the
/// answer is a named outcome rather than a database error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bot_another_application_has_is_taken(pool: PgPool) {
    let first = insert_application_account(&pool, "one").await;
    let second = insert_application_account(&pool, "two").await;

    bind_bot(&pool, first, BOT)
        .await
        .expect("the first binding");

    assert!(matches!(
        bind_bot(&pool, second, BOT).await,
        Err(BindError::Taken)
    ));

    // And the first application keeps it: a refused binding is a write that did not
    // happen rather than one that half happened.
    assert_eq!(discord_id_of(&pool, first).await, Some(BOT));
    assert_eq!(discord_id_of(&pool, second).await, None);
}

/// Binding the same application's account again is not a conflict with itself.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_twice_is_not_a_conflict(pool: PgPool) {
    let account = insert_application_account(&pool, "one").await;

    bind_bot(&pool, account, BOT)
        .await
        .expect("the first binding");
    assert!(matches!(bind_bot(&pool, account, BOT).await, Ok(())));
    assert_eq!(discord_id_of(&pool, account).await, Some(BOT));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_merges_balances_history_claims_metadata_and_mutes(pool: PgPool) {
    use serde_json::json;
    use support::*;
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, money.user1, "recipient").await;
    let destination = account_of(&pool, application).await;
    insert_asset(&pool, destination, money.currency, 100).await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 1).await.unwrap();
    let source = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap()
        .id;
    insert_asset(&pool, source, money.currency2, 7).await;
    vc_core::issue::issue(&pool, money.guild, BOT, Some(5))
        .await
        .unwrap();
    vc_core::payment::pay(&pool, source, money.user2, "n", 1)
        .await
        .unwrap();

    let incoming = vc_core::claim::create(&pool, 1, BOT, "n", 2, Some(json!({"incoming": "yes"})))
        .await
        .unwrap();
    let outgoing = vc_core::claim::create(
        &pool,
        source,
        money.user2,
        "n",
        3,
        Some(json!({"outgoing": "yes"})),
    )
    .await
    .unwrap();
    let self_claim = vc_core::claim::create(
        &pool,
        destination,
        BOT,
        "n",
        4,
        Some(json!({"self": "yes"})),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO mutes (user_id, muted_user_id, inserted_at) VALUES (2, $1, now()), (2, $2, now())")
        .bind(source).bind(destination).execute(&pool).await.unwrap();
    vc_core::mute::mute_currency(&pool, source, "n", time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    sqlx::query("INSERT INTO payments_idempotency (user_id, idempotency_key, expires, inserted_at, updated_at)
        VALUES ($1, 'same', now() + interval '1 day', now(), now()), ($2, 'same', now() + interval '1 day', now(), now())")
        .bind(i64::from(source)).bind(i64::from(destination)).execute(&pool).await.unwrap();

    bind_bot(&pool, destination, BOT).await.unwrap();
    assert_eq!(discord_id_of(&pool, destination).await, Some(BOT));
    assert!(
        vc_core::user::find_by_id(&pool, source)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(get_amount(&pool, BOT, money.currency).await, 105);
    assert_eq!(get_amount(&pool, BOT, money.currency2).await, 7);
    let history: Vec<(i64, i64)> =
        sqlx::query_as("SELECT sender_id, receiver_id FROM currency_payment_histories ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        history,
        vec![(2, i64::from(destination)), (i64::from(destination), 2)]
    );
    let issued_to: i64 = sqlx::query_scalar("SELECT receiver_id FROM currency_given_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(issued_to, i64::from(destination));
    for (id, operator, claimant, payer, metadata) in [
        (incoming, 1, 1, destination, json!({"incoming": "yes"})),
        (
            outgoing,
            destination,
            destination,
            2,
            json!({"outgoing": "yes"}),
        ),
        (
            self_claim,
            destination,
            destination,
            destination,
            json!({"self": "yes"}),
        ),
    ] {
        let claim = vc_core::claim::view(&pool, operator, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((claim.claimant.id, claim.payer.id), (claimant, payer));
        assert_eq!(claim.metadata, metadata);
        assert_eq!(claim.status.as_deref(), Some("pending"));
    }
    let mutes: Vec<(i32, Option<i64>, Option<i32>)> =
        sqlx::query_as("SELECT user_id, currency_id, muted_user_id FROM mutes ORDER BY user_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        mutes,
        vec![
            (2, None, Some(destination)),
            (destination, Some(money.currency), None)
        ]
    );
    let keys: Vec<i64> = sqlx::query_scalar("SELECT user_id FROM payments_idempotency")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(keys, vec![i64::from(destination)]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn failed_binding_rolls_back_the_entire_merge(pool: PgPool) {
    let money = support::setup_money(&pool).await;
    let application = support::insert_application(&pool, money.user1, "rollback").await;
    let destination = support::account_of(&pool, application).await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 10).await.unwrap();
    let source = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap()
        .id;
    let claim = vc_core::claim::create(
        &pool,
        1,
        BOT,
        "n",
        1,
        Some(serde_json::json!({"keep": "yes"})),
    )
    .await
    .unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION reject_bind() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'reject final binding'; END $$;
        CREATE TRIGGER reject_bind BEFORE UPDATE OF discord_id ON users
        FOR EACH ROW EXECUTE FUNCTION reject_bind();",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        bind_bot(&pool, destination, BOT).await,
        Err(BindError::Database(_))
    ));
    assert_eq!(discord_id_of(&pool, destination).await, None);
    assert_eq!(discord_id_of(&pool, source).await, Some(BOT));
    assert_eq!(support::get_amount(&pool, BOT, money.currency).await, 10);
    let still = vc_core::claim::view(&pool, 1, claim)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.payer.id, source);
    assert_eq!(still.metadata, serde_json::json!({"keep": "yes"}));
    let receiver: i64 = sqlx::query_scalar("SELECT receiver_id FROM currency_payment_histories")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(receiver, i64::from(source));
}

async fn wait_for_lock(pool: &PgPool, query: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND wait_event_type='Lock'
                AND query LIKE $1)",
            )
            .bind(query)
            .fetch_one(pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_includes_a_payment_that_finishes_while_it_waits(pool: PgPool) {
    let money = support::setup_money(&pool).await;
    let application = support::insert_application(&pool, money.user1, "concurrent payment").await;
    let destination = support::account_of(&pool, application).await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 1).await.unwrap();
    let mut payment = pool.begin().await.unwrap();
    vc_core::payment::pay_in(&mut payment, 2, BOT, "n", 20)
        .await
        .unwrap();
    let other = pool.clone();
    let mut binding = tokio::spawn(async move { bind_bot(&other, destination, BOT).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut binding)
            .await
            .is_err()
    );
    payment.commit().await.unwrap();
    binding.await.unwrap().unwrap();
    assert_eq!(support::get_amount(&pool, BOT, money.currency).await, 21);
    assert_eq!(
        support::get_amount(&pool, money.user2, money.currency).await,
        979
    );
    let receivers: Vec<i64> =
        sqlx::query_scalar("SELECT receiver_id FROM currency_payment_histories")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(receivers, vec![i64::from(destination); 2]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn concurrent_connections_only_merge_once(pool: PgPool) {
    let money = support::setup_money(&pool).await;
    let first = insert_application_account(&pool, "first").await;
    let second = insert_application_account(&pool, "second").await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 10).await.unwrap();
    let source = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap()
        .id;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE")
        .bind(source)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let other = pool.clone();
    let mut first_binding = tokio::spawn(async move { bind_bot(&other, first, BOT).await });
    let other = pool.clone();
    let second_binding = tokio::spawn(async move { bind_bot(&other, second, BOT).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut first_binding)
            .await
            .is_err()
    );
    blocker.commit().await.unwrap();
    let (a, b) = (first_binding.await.unwrap(), second_binding.await.unwrap());
    assert!(
        matches!(
            (&a, &b),
            (Ok(()), Err(BindError::Taken)) | (Err(BindError::Taken), Ok(()))
        ),
        "{a:?}, {b:?}"
    );
    let bound = vc_core::user::find_by_discord_id(&pool, BOT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.id, if a.is_ok() { first } else { second });
    assert_eq!(support::get_amount(&pool, BOT, money.currency).await, 10);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn binding_does_not_block_a_resolved_payment_from_the_application(pool: PgPool) {
    let money = support::setup_money(&pool).await;
    let application = support::insert_application(&pool, money.user1, "payer").await;
    let destination = support::account_of(&pool, application).await;
    support::insert_asset(&pool, destination, money.currency, 100).await;
    vc_core::payment::pay(&pool, 2, BOT, "n", 1).await.unwrap();
    let mut payment = pool.begin().await.unwrap();
    vc_core::user::insert_if_not_exists(&mut payment, BOT)
        .await
        .unwrap();
    let other = pool.clone();
    let mut binding = tokio::spawn(async move { bind_bot(&other, destination, BOT).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut binding)
            .await
            .is_err()
    );
    // The binding must give up its destination lock while the payment holds
    // a reference to the source; otherwise each would wait for the other.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        vc_core::payment::pay_in(&mut payment, destination, BOT, "n", 20),
    )
    .await
    .unwrap()
    .unwrap();
    payment.commit().await.unwrap();
    binding.await.unwrap().unwrap();
    assert_eq!(support::get_amount(&pool, BOT, money.currency).await, 101);
}

async fn payment_during_merge(pool: PgPool, bulk: bool, rebind: bool) {
    let money = support::setup_money(&pool).await;
    let application = support::insert_application(&pool, money.user1, "concurrent receiver").await;
    let destination = support::account_of(&pool, application).await;
    let receiver = if rebind { BOT + 1 } else { BOT };
    if rebind {
        bind_bot(&pool, destination, receiver).await.unwrap();
    }
    vc_core::payment::pay(&pool, 2, BOT, "n", 1).await.unwrap();
    sqlx::raw_sql(
        "CREATE FUNCTION pause_merge() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN PERFORM pg_advisory_xact_lock(7131313); RETURN OLD; END $$;
        CREATE TRIGGER pause_merge BEFORE DELETE ON assets
        FOR EACH ROW EXECUTE FUNCTION pause_merge();",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(7131313)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let other = pool.clone();
    let binding = tokio::spawn(async move { bind_bot(&other, destination, BOT).await });
    wait_for_lock(&pool, "%DELETE FROM assets WHERE user_id%").await;
    let other = pool.clone();
    let payment = tokio::spawn(async move {
        if bulk {
            vc_core::payment::pay_bulk(
                &other,
                2,
                &[vc_core::payment::BulkPayment {
                    unit: "n".into(),
                    receiver_discord_id: receiver,
                    amount: 20,
                }],
            )
            .await
        } else {
            vc_core::payment::pay(&other, 2, receiver, "n", 20).await
        }
    });
    wait_for_lock(&pool, "SELECT id, discord_id%FOR KEY SHARE").await;
    blocker.commit().await.unwrap();
    binding.await.unwrap().unwrap();
    payment.await.unwrap().unwrap();
    assert_eq!(
        support::get_amount(&pool, BOT, money.currency).await,
        if rebind { 1 } else { 21 }
    );
    assert_eq!(
        support::get_amount(&pool, money.user2, money.currency).await,
        979
    );
    let receivers: Vec<i64> =
        sqlx::query_scalar("SELECT receiver_id FROM currency_payment_histories ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    if rebind {
        let old_bot = vc_core::user::find_by_discord_id(&pool, receiver)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(old_bot.id, destination);
        assert_eq!(
            support::get_amount(&pool, receiver, money.currency).await,
            20
        );
        assert_eq!(
            receivers,
            vec![i64::from(destination), i64::from(old_bot.id)]
        );
    } else {
        assert_eq!(receivers, vec![i64::from(destination); 2]);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_waiting_for_a_merge_uses_the_surviving_account(pool: PgPool) {
    payment_during_merge(pool, false, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bulk_payment_waiting_for_a_merge_uses_the_surviving_account(pool: PgPool) {
    payment_during_merge(pool, true, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_bulk_payment_to_the_previous_bot_survives_a_rebind(pool: PgPool) {
    payment_during_merge(pool, true, true).await;
}
