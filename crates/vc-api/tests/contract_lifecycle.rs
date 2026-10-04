mod support;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn settled_contracts_are_not_listed_as_open(pool: PgPool) {
    let owner = 500_000_000_000_000_001;
    let party = 100_000_000_000_000_001;
    support::insert_user(&pool, 1, owner).await;
    support::insert_user(&pool, 2, party).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", 900_000_000_000_000_001, 500).await;
    support::insert_asset(&pool, 2, 1, 1000).await;
    let application = support::insert_application(&pool, owner, "an application").await;
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    let id = vc_core::contract::create(
        &pool,
        application,
        "nyan",
        &[vc_core::contract::NewParty {
            discord_id: party,
            amount: 100,
        }],
        None,
        Some(60),
        past,
    )
    .await
    .unwrap();
    vc_core::contract::approve(&pool, id, 2, || past)
        .await
        .unwrap();
    let state = support::state(pool.clone(), support::fake());
    vc_api::scheduler::settle_expired(&state).await;
    let contract = vc_core::contract::find(&pool, id).await.unwrap().unwrap();
    assert_eq!(contract.status, "expired");
    assert_eq!(contract.remaining, 0);
    let response = vc_api::command::contract::handle(
        &state,
        json!({"subcommand":"list"}).as_object().unwrap(),
        &json!({"user":{"id":party.to_string()}}),
    )
    .await
    .unwrap();
    let open = vc_core::contract::open_of_party(&pool, party, 1, 5)
        .await
        .unwrap();
    assert!(
        response
            .to_string()
            .contains("あなたが対象の契約はありません")
    );
    assert!(open.contracts.is_empty());
    assert_eq!(
        open.total, 0,
        "settled contract still appears in the command: {response}"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_payments_draw_from_the_first_approval_first(pool: PgPool) {
    let owner = 500_000_000_000_000_001;
    let alice = 100_000_000_000_000_001;
    let bob = 100_000_000_000_000_002;
    support::insert_user(&pool, 1, owner).await;
    support::insert_user(&pool, 2, alice).await;
    support::insert_user(&pool, 3, bob).await;
    support::insert_currency(&pool, 1, "nyan", "nyan", 900_000_000_000_000_001, 500).await;
    support::insert_asset(&pool, 2, 1, 1000).await;
    support::insert_asset(&pool, 3, 1, 1000).await;
    let application = support::insert_application(&pool, owner, "an application").await;
    let now = time::OffsetDateTime::now_utc();
    let parties = [
        vc_core::contract::NewParty {
            discord_id: alice,
            amount: 100,
        },
        vc_core::contract::NewParty {
            discord_id: bob,
            amount: 100,
        },
    ];
    let id = vc_core::contract::create(&pool, application, "nyan", &parties, None, None, now)
        .await
        .unwrap();
    vc_core::contract::approve(&pool, id, 3, || now)
        .await
        .unwrap();
    vc_core::contract::approve(&pool, id, 2, || now)
        .await
        .unwrap();
    vc_core::contract::pay(&pool, id, application, owner, None, 20, || {
        now + time::Duration::seconds(20)
    })
    .await
    .unwrap();
    // A partial payment changes updated_at, and a repeated approval must not
    // move the party behind a later approval.
    assert!(
        !vc_core::contract::approve(&pool, id, 3, || now + time::Duration::seconds(21))
            .await
            .unwrap()
    );
    vc_core::contract::pay(&pool, id, application, owner, None, 130, || {
        now + time::Duration::seconds(22)
    })
    .await
    .unwrap();
    // The two approvals' locks first, then the money the charges drew: the lock
    // names the party whose account it left, and a charge names the party it came
    // out of. (A return row would have a NULL sender, but none has happened yet.)
    let history: Vec<(i64, i64)> = sqlx::query_as("SELECT sender_id, amount FROM currency_payment_histories WHERE contract_id = $1 ORDER BY id")
        .bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(history, vec![(3, 100), (2, 100), (3, 20), (3, 80), (2, 50)]);
    vc_core::contract::withdraw(&pool, id, 2, || now + time::Duration::seconds(30))
        .await
        .unwrap();
    let alice_balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = 2 AND currency_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let bob_balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = 3 AND currency_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (alice_balance, bob_balance),
        (950, 900),
        "Bob approved first; his remainder should pay first"
    );
}
