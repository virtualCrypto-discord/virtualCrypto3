//! A lock is a transfer, not a deletion.
//!
//! A party approving a contract is debited, and the same amount arrives in the
//! contract's own account — a `users` row named by `users.contract_id`, which is
//! what `assets` can point at. The bug this file is here for was the other thing:
//! the amount left `assets` and survived only as a number in
//! `contract_parties.remaining`, which made the currency's supply — the sum of
//! every balance in it — shrink by every lock, and made a pool's allowance
//! measure a supply that was not there.

mod support;

use sqlx::PgPool;
use support::{insert_application, insert_asset, insert_currency, insert_user};
use vc_core::contract::{NewParty, approve, create, withdraw};

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const PARTY: i32 = 2;
const PARTY_DISCORD_ID: i64 = 100_000_000_000_000_001;
const GUILD: i64 = 900_000_000_000_000_001;

async fn fixture(pool: &PgPool) -> i64 {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, PARTY, PARTY_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, PARTY, 1, 1_000).await;

    insert_application(pool, OWNER_DISCORD_ID, "an application").await
}

/// Everything the currency has, which is what must not move when a party locks
/// something: a lock changes whose balance it is, not how much exists.
async fn supply(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(
        "SELECT COALESCE(SUM(amount), 0)::bigint AS \"total!\" FROM assets WHERE currency_id = 1"
    )
    .fetch_one(pool)
    .await
    .expect("the supply")
}

/// What the contract holds, which is what the table behind `users.contract_id`
/// is for.
async fn escrow(pool: &PgPool, contract_id: i64) -> i64 {
    sqlx::query_scalar!(
        "SELECT COALESCE(SUM(a.amount), 0)::bigint AS \"total!\"
           FROM assets a
           JOIN users u ON u.id = a.user_id
          WHERE u.contract_id = $1",
        contract_id
    )
    .fetch_one(pool)
    .await
    .expect("the escrow")
}

async fn locked(pool: &PgPool, application: i64) -> i64 {
    create(
        pool,
        application,
        "nyan",
        &[NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        None,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract")
}

/// The one that matters: approving puts the money in the contract's account and
/// takes it out of nobody's supply.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_lock_moves_money_into_the_contract(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = locked(&pool, application).await;

    let before = supply(&pool).await;

    approve(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("an approval");

    assert_eq!(supply(&pool).await, before, "the supply did not move");
    assert_eq!(escrow(&pool, id).await, 100, "the contract holds it");
}

/// Two parties, two transfers: the account is the sum of what each of them
/// locked, which is what the remainders add up to.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_account_holds_what_the_parties_locked(pool: PgPool) {
    let application = fixture(&pool).await;
    insert_user(&pool, 4, 200_000_000_000_000_001).await;
    insert_asset(&pool, 4, 1, 1_000).await;

    let id = create(
        &pool,
        application,
        "nyan",
        &[
            NewParty {
                discord_id: PARTY_DISCORD_ID,
                amount: 100,
            },
            NewParty {
                discord_id: 200_000_000_000_000_001,
                amount: 250,
            },
        ],
        None,
        Some(3_600),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract");

    let before = supply(&pool).await;

    approve(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("the first approval");
    approve(&pool, id, 4, time::OffsetDateTime::now_utc())
        .await
        .expect("the second approval");

    assert_eq!(escrow(&pool, id).await, 350, "both locks are in it");
    assert_eq!(
        supply(&pool).await,
        before,
        "and the supply is where it was"
    );
}

/// The end of a contract empties the account as the money goes home — the
/// remainder is the account's whole balance, so it lands at nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_end_of_a_contract_empties_the_account(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = locked(&pool, application).await;

    let before = supply(&pool).await;

    approve(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("an approval");

    withdraw(&pool, id, PARTY, time::OffsetDateTime::now_utc())
        .await
        .expect("a withdrawal");

    assert_eq!(escrow(&pool, id).await, 0, "nothing is held any more");
    assert_eq!(supply(&pool).await, before, "and the supply never moved");
}
