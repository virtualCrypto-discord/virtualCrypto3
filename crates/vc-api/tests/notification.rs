//! Claim-update notifications, ported from
//! `test/virtualCrypto/notification/single_test.exs`.
//!
//! The Elixir tests use `VirtualCryptoTest.Notification.Sink`, a `Notification`
//! implementation that sends what it receives to the test process; this records
//! the same deliveries instead, so the assertions are on the payload that would
//! reach an application.

mod support;

use std::sync::Mutex;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{Money, insert_claim, insert_claim_metadata, setup_money};
use vc_core::claim::Transition;
use vc_core::notification::Notifier;

const CLAIM_ID: i64 = 1;
const AMOUNT: i64 = 100;

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
}

/// A pending claim of 100 from user2 to user1, with the claimant's metadata when
/// one is given.
async fn fixture(pool: &PgPool, metadata: Option<Value>) -> Money {
    let money = setup_money(pool).await;
    insert_claim(pool, CLAIM_ID, AMOUNT, "pending", 1, 2, money.currency).await;

    if let Some(metadata) = metadata {
        insert_claim_metadata(pool, CLAIM_ID, 1, 2, 1, metadata).await;
    }

    money
}

/// `format_claim_for_notification/1`, with the claimant's metadata.
fn event(money: &Money, status: &str, metadata: Value) -> Value {
    json!({
        "id": CLAIM_ID,
        "status": status,
        "amount": AMOUNT.to_string(),
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
            .all(|field| field.chars().all(|c| c.is_ascii_digit())),
        "numeric fields: {text}"
    );
}

/// Compares the delivered event, ignoring `updated_at`, which is checked
/// separately because it is the transition's own timestamp.
fn assert_event(actual: &Value, expected: &Value) {
    assert_timestamp(&actual["updated_at"]);

    let mut actual = actual.clone();
    let mut expected = expected.clone();
    actual["updated_at"] = Value::Null;
    expected["updated_at"] = Value::Null;

    assert_eq!(actual, expected);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_claim_notifies_the_claimant(pool: PgPool) {
    let money = fixture(&pool, None).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        CLAIM_ID,
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
    assert_event(&events[0], &event(&money, "approved", json!({})));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_a_claim_notifies_the_claimant(pool: PgPool) {
    let money = fixture(&pool, None).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        CLAIM_ID,
        Transition::Denied,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);

    let (claimant_id, events) = &deliveries[0];
    assert_eq!(*claimant_id, 1);
    assert_eq!(events.len(), 1);
    assert_event(&events[0], &event(&money, "denied", json!({})));
}

/// The metadata in the event is the claimant's own row, not the operator's.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_claim_carries_the_claimants_metadata(pool: PgPool) {
    let money = fixture(&pool, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        CLAIM_ID,
        Transition::Approved,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_event(
        &deliveries[0].1[0],
        &event(&money, "approved", json!({ "x": "y" })),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_a_claim_carries_the_claimants_metadata(pool: PgPool) {
    let money = fixture(&pool, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(
        &pool,
        &sink,
        2,
        CLAIM_ID,
        Transition::Denied,
        Some(json!({ "a": "b" })),
    )
    .await
    .expect("the transition succeeds");

    let deliveries = sink.take();
    assert_eq!(deliveries.len(), 1);
    assert_event(
        &deliveries[0].1[0],
        &event(&money, "denied", json!({ "x": "y" })),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn cancelling_a_claim_notifies_nobody(pool: PgPool) {
    fixture(&pool, Some(json!({ "x": "y" }))).await;
    let sink = Sink::default();

    vc_core::claim::transition(&pool, &sink, 1, CLAIM_ID, Transition::Canceled, None)
        .await
        .expect("the transition succeeds");

    assert!(sink.take().is_empty(), "nothing is delivered");
}
