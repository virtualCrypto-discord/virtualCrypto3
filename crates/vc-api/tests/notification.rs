//! Claim-update notifications, ported from
//! `test/virtualCrypto/notification/single_test.exs` and `bulk_test.exs`.
//!
//! The Elixir tests use `VirtualCryptoTest.Notification.Sink`, a `Notification`
//! implementation that sends what it receives to the test process; this records
//! the same deliveries instead, so the assertions are on the payload that would
//! reach an application.

mod support;

use std::sync::Mutex;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Money, insert_claim, insert_claim_metadata, insert_user, setup_money};
use vc_core::claim::{PartialClaim, Transition};
use vc_core::notification::Notifier;

const CURRENCY: i64 = 1;
/// user1 is account 1 and user2 is account 2; 123 is an account the tests make.
const THIRD_ACCOUNT: i32 = 3;
const THIRD_DISCORD_ID: i64 = 123;

/// `VirtualCryptoTest.Notification.Sink`.
#[derive(Default)]
struct Sink {
    deliveries: Mutex<Vec<(i32, Vec<Value>)>>,
}

impl Sink {
    /// The deliveries recorded so far, in order.
    fn take(&self) -> Vec<(i32, Vec<Value>)> {
        std::mem::take(&mut self.deliveries.lock().expect("the sink is not poisoned"))
    }
}

impl Notifier for Sink {
    fn notify_claim_update(&self, claimant_id: i32, events: &[Value]) {
        self.deliveries
            .lock()
            .expect("the sink is not poisoned")
            .push((claimant_id, events.to_vec()));
    }

    fn notify_grant_decided(&self, _application_id: i64, _guild_id: i64) {
        // Claims only: grant decisions go through the webhook path, which has
        // its own test.
    }

    fn notify_contract_decided(&self, _application_id: i64, _contract_id: i64) {
        // The same, and a contract decision's own test is
        // `tests/contract_notification.rs`.
    }
}

/// `VirtualCryptoTest.Notification.Setup.setup_claim/1`: a pending claim, with
/// the claimant's metadata when one is given.
async fn pending_claim(
    pool: &PgPool,
    id: i64,
    amount: i64,
    claimant: i32,
    payer: i32,
    metadata: Option<Value>,
) {
    insert_claim(pool, id, amount, "pending", claimant, payer, CURRENCY).await;

    if let Some(metadata) = metadata {
        insert_claim_metadata(pool, id, claimant, payer, claimant, metadata).await;
    }
}

/// `format_claim_for_notification/1`, with the claimant's metadata. The payer is
/// user2 in every one of these tests.
fn expected(money: &Money, id: i64, status: &str, amount: i64, metadata: Value) -> Value {
    json!({
        "id": id,
        "status": status,
        "amount": amount.to_string(),
        "updated_at": Value::Null,
        "metadata": metadata,
        "payer": {
            "id": 2,
            "discord": { "id": support::MONEY_USER2.to_string() },
        },
        "currency": {
            "id": money.currency,
            "unit": money.unit,
            "name": money.name,
            "guild": money.guild.to_string(),
            "pool_amount": "500",
        },
    })
}

/// Jason writes a UTC `DateTime` at second precision with a trailing `Z`; the
/// Elixir tests assert exactly that type and zone.
fn assert_timestamp(value: &Value) {
    let text = value.as_str().expect("a string");

    assert_eq!(text.len(), 20, "second precision: {text}");
    assert!(text.ends_with('Z'), "UTC: {text}");
    assert_eq!(text.as_bytes()[10], b'T', "an ISO 8601 separator: {text}");

    let digits: Vec<&str> = text[..19].split(['-', 'T', ':']).collect();
    assert_eq!(digits.len(), 6, "date and time fields: {text}");
    assert!(
        digits
            .iter()
            .all(|field| field.chars().all(|character| character.is_ascii_digit())),
        "numeric fields: {text}"
    );
}

/// The events as comparable strings, with `updated_at` checked separately
/// because it is the transition's own timestamp. The Elixir tests compare sets,
/// since the order the events arrive in is the database's.
fn canonical(events: &[Value]) -> Vec<String> {
    let mut rendered: Vec<String> = events
        .iter()
        .map(|event| {
            // A delivered event carries the timestamp, which has to be the form
            // the Elixir tests assert; an expected one carries null.
            if !event["updated_at"].is_null() {
                assert_timestamp(&event["updated_at"]);
            }

            let mut event = event.clone();
            event["updated_at"] = Value::Null;

            event.to_string()
        })
        .collect();
    rendered.sort();

    rendered
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_claim_notifies_the_claimant(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, None).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        1,
        Transition::Approved,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1, "one delivery");

    let (claimant_id, events) = &deliveries[0];
    assert_eq!(*claimant_id, 1, "the claimant is user 1");

    let user = vc_core::user::find_by_id(&pool, *claimant_id)
        .await
        .expect("a lookup")
        .expect("the claimant exists");
    assert_eq!(user.discord_id, Some(support::MONEY_USER1));

    assert_eq!(events.len(), 1);
    assert_eq!(
        canonical(events),
        canonical(&[expected(&money, 1, "approved", 100, json!({}))])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_a_claim_notifies_the_claimant(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, None).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        1,
        Transition::Denied,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "denied", 100, json!({}))])
    );
}

/// The metadata in the event is the claimant's own row, not the operator's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_claim_carries_the_claimants_metadata(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        1,
        Transition::Approved,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "approved", 100, json!({ "x": "y" }))])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_a_claim_carries_the_claimants_metadata(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        1,
        Transition::Denied,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "denied", 100, json!({ "x": "y" }))])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancelling_a_claim_notifies_nobody(pool: PgPool) {
    setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(&pool, &sink, 1, 1, Transition::Canceled, None)
        .await
        .expect("the transition succeeds");

    assert!(sink.take().is_empty(), "nothing is delivered");
}

/// Approving several of one claimant's claims delivers once, carrying both.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_several_claims_notifies_the_claimant_once(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, 1, 2, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("approved".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("approved".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1, "one delivery for the one claimant");
    assert_eq!(deliveries[0].0, 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[
            expected(&money, 1, "approved", 100, json!({ "x": "y" })),
            expected(&money, 2, "approved", 200, json!({})),
        ])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_several_claims_notifies_the_claimant_once(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, 1, 2, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("denied".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("denied".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[
            expected(&money, 1, "denied", 100, json!({ "x": "y" })),
            expected(&money, 2, "denied", 200, json!({})),
        ])
    );
}

/// One claimant can be told about an approval and a denial in the same batch.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_and_denying_together_notifies_the_claimant_once(pool: PgPool) {
    let money = setup_money(&pool).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, 1, 2, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("approved".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("denied".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[
            expected(&money, 1, "approved", 100, json!({ "x": "y" })),
            expected(&money, 2, "denied", 200, json!({})),
        ])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_claims_of_different_claimants_notifies_each(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, THIRD_ACCOUNT, THIRD_DISCORD_ID).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, THIRD_ACCOUNT, 2, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("approved".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("approved".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let mut deliveries = sink.take();
    deliveries.sort_by_key(|(claimant_id, _)| *claimant_id);
    assert_eq!(deliveries.len(), 2, "one delivery per claimant");
    assert_eq!(deliveries[0].0, 1);
    assert_eq!(deliveries[1].0, THIRD_ACCOUNT);

    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "approved", 100, json!({ "x": "y" }))])
    );
    assert_eq!(
        canonical(&deliveries[1].1),
        canonical(&[expected(&money, 2, "approved", 200, json!({}))])
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_claims_of_different_claimants_notifies_each(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, THIRD_ACCOUNT, THIRD_DISCORD_ID).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, THIRD_ACCOUNT, 2, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("denied".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("denied".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let mut deliveries = sink.take();
    deliveries.sort_by_key(|(claimant_id, _)| *claimant_id);
    assert_eq!(deliveries.len(), 2);
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "denied", 100, json!({ "x": "y" }))])
    );
    assert_eq!(
        canonical(&deliveries[1].1),
        canonical(&[expected(&money, 2, "denied", 200, json!({}))])
    );
}

/// Approving, denying and cancelling in one batch: a cancellation tells nobody,
/// so the third claim is absent from the deliveries.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_mixed_batch_notifies_only_the_changed_hands(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, THIRD_ACCOUNT, THIRD_DISCORD_ID).await;
    pending_claim(&pool, 1, 100, 1, 2, Some(json!({ "x": "y" }))).await;
    pending_claim(&pool, 2, 200, THIRD_ACCOUNT, 2, None).await;
    // The operator is this claim's claimant, which is what a cancellation
    // requires.
    pending_claim(&pool, 3, 100, 2, 1, None).await;

    let sink = Sink::default();
    vc_core::claim::update_claims(
        &pool,
        &sink,
        2,
        &[
            PartialClaim {
                id: 1,
                status: Some("approved".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 2,
                status: Some("denied".to_string()),
                metadata: None,
            },
            PartialClaim {
                id: 3,
                status: Some("canceled".to_string()),
                metadata: None,
            },
        ],
    )
    .await
    .expect("the update succeeds");

    let mut deliveries = sink.take();
    deliveries.sort_by_key(|(claimant_id, _)| *claimant_id);
    assert_eq!(deliveries.len(), 2, "the cancellation tells nobody");
    assert_eq!(
        canonical(&deliveries[0].1),
        canonical(&[expected(&money, 1, "approved", 100, json!({ "x": "y" }))])
    );
    assert_eq!(
        canonical(&deliveries[1].1),
        canonical(&[expected(&money, 2, "denied", 200, json!({}))])
    );
}

async fn approval_under_pool_pressure(pool: PgPool, bulk: bool) {
    support::insert_user(&pool, 1, 100000000000000001).await;
    support::insert_user(&pool, 2, 100000000000000002).await;
    support::insert_currency(&pool, 1, "test", "tst", 900000000000000001, 0).await;
    support::insert_asset(&pool, 1, 1, 0).await;
    support::insert_asset(&pool, 2, 1, 1000).await;
    support::insert_claim(&pool, 1, 500, "pending", 1, 2, 1).await;

    // Hold the released transaction connection briefly, reproducing pool pressure
    // between committing the payment and acquiring a connection for its notification.
    let pressured = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(100))
        .after_release(|_, _| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                Ok(true)
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let sink = Sink::default();
    if bulk {
        vc_core::claim::update_claims(
            &pressured,
            &sink,
            2,
            &[PartialClaim {
                id: 1,
                status: Some("approved".into()),
                metadata: None,
            }],
        )
        .await
        .expect("bulk approval must not fail after committing");
    } else {
        vc_core::claim::transition(&pressured, &sink, 2, 1, Transition::Approved, None)
            .await
            .expect("approval must not fail after committing");
    }
    let status: String = sqlx::query_scalar("SELECT status::text FROM claims WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = 2 AND currency_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(balance, 500);
    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].0, 1);
    assert_eq!(deliveries[0].1[0]["status"], "approved");
    assert_eq!(deliveries[0].1[0]["amount"], "500");
    pressured.close().await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approval_does_not_reacquire_after_commit(pool: PgPool) {
    approval_under_pool_pressure(pool, false).await;
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bulk_approval_does_not_reacquire_after_commit(pool: PgPool) {
    approval_under_pool_pressure(pool, true).await;
}
