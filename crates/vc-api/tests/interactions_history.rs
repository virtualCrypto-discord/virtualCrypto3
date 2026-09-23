//! `/history`: the two ledgers, read back.
//!
//! An addition rather than a port — the Elixir writes both tables and reads neither, and no
//! command of its names a history — so nothing here has an Elixir case behind it:
//! `docs/known-gaps.md` is the design, and this file holds it to the two things that file
//! claims. What may be seen follows what may be done, and what a row says is what happened:
//! a payment names both sides, and a contract's charge names the application it paid.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    MONEY_GUILD, MONEY_USER1, MONEY_USER2, Response, execute_from_dm, execute_from_guild, fake,
    from_guild, insert_user, interaction, setup_money, state,
};

/// Somebody the money fixture does not have: a second person to pay and be paid by, so that a
/// filter can take away some of a ledger rather than all of it.
const OTHER: i64 = 100_000_000_000_000_003;

/// No permissions at all, which is every member who is not an administrator.
const NO_PERMISSIONS: &str = "0";

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// A `/history` interaction, typed in a direct message — which is where `/history pay` lives and
/// where `/history issue` is refused.
fn history(user: i64, subcommand: &str, options: Vec<Value>) -> Value {
    execute_from_dm(
        json!({
            "name": "history",
            "options": [{ "name": subcommand, "type": 1, "options": options }],
        }),
        user,
    )
}

/// The chosen option of a subcommand, as Discord sends it.
fn option(name: &str, value: Value) -> Value {
    json!({ "name": name, "value": value })
}

/// `/history issue`, in a guild, by the given permissions.
fn issue_in_guild(user: i64, permissions: &str, options: Vec<Value>) -> Value {
    from_guild(
        json!({
            "name": "history",
            "options": [{ "name": "issue", "type": 1, "options": options }],
        }),
        user,
        MONEY_GUILD,
        permissions,
    )
}

/// Everything one answer says, as one string: what a person reads.
fn rendered(response: &Response) -> String {
    response.body["data"].to_string()
}

/// The custom id of the arrow that moves forward, taken from the screen rather than rebuilt: the
/// filter it carries is the thing under test, so the test cannot supply it.
fn next_button(response: &Response) -> String {
    let row = response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .find(|child| child["type"] == 1 && child["components"].is_array())
        .expect("the row of arrows");

    row["components"]
        .as_array()
        .expect("four arrows")
        .iter()
        .filter_map(|arrow| arrow["custom_id"].as_str())
        // On the first page of two the only enabled arrows are the two that move forward, and
        // the first of them is the one this asks for.
        .find(|id| !id.starts_with("disabled-"))
        .expect("an arrow that goes somewhere")
        .to_owned()
}

/// One payment through the path `/pay` uses, so the ledger is written the way the service writes
/// it rather than by hand.
async fn pay(pool: &PgPool, sender: i64, receiver: i64, unit: &str, amount: i64) {
    vc_core::payment::pay_from_discord(pool, sender, receiver, unit, amount)
        .await
        .expect("a payment");
}

/// The same for the pool, through the path `/issue` uses.
async fn issue(pool: &PgPool, receiver: i64, amount: i64) {
    vc_core::issue::issue(pool, MONEY_GUILD, receiver, Some(amount))
        .await
        .expect("an issuance");
}

/// Both directions on one screen: what the caller paid is a 送金 and what they were paid is a
/// 受取, and a ledger that showed only one of them would be half a ledger.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pay_shows_both_what_was_sent_and_what_came_in(pool: PgPool) {
    let money = setup_money(&pool).await;

    pay(&pool, MONEY_USER2, MONEY_USER1, &money.unit, 500).await;
    pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 300).await;

    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER2, "pay", vec![])).await);

    assert!(screen.contains("**送金の履歴** (2件)"), "{screen}");
    assert!(screen.contains("送金: **500**"), "{screen}");
    assert!(screen.contains("受取: **300**"), "{screen}");
    // The other side of each row, which is who the money went to and came from.
    assert!(screen.contains(&MONEY_USER1.to_string()), "{screen}");

    // And the ledger is personal: the other party sees the same two rows from their own side,
    // and somebody who was not in them sees nothing.
    insert_user(&pool, 9, OTHER).await;

    let theirs =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(theirs.contains("受取: **500**"), "{theirs}");
    assert!(theirs.contains("送金: **300**"), "{theirs}");

    let stranger =
        rendered(&interaction(router(pool.clone()), history(OTHER, "pay", vec![])).await);
    assert!(stranger.contains("送金の履歴はありません"), "{stranger}");
}

/// The count is the whole ledger and the arrows move through it: five rows on the first page,
/// the rest on the second, and the page says how many there are altogether.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_count_is_the_whole_ledger_and_the_arrows_move(pool: PgPool) {
    let money = setup_money(&pool).await;

    for _ in 0..6 {
        pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 10).await;
    }

    let first = interaction(router(pool.clone()), history(MONEY_USER2, "pay", vec![])).await;
    let shown = rendered(&first);
    assert!(shown.contains("**送金の履歴** (6件)"), "{shown}");
    assert_eq!(
        shown.matches("受取: **10**").count(),
        5,
        "a page is five rows: {shown}"
    );

    let pressed = interaction(
        router(pool.clone()),
        support::button_from_guild(json!({ "custom_id": next_button(&first) }), MONEY_USER2),
    )
    .await;

    let second = rendered(&pressed);
    assert!(second.contains("**送金の履歴** (6件)"), "{second}");
    assert_eq!(
        second.matches("受取: **10**").count(),
        1,
        "the last page holds the one row left: {second}"
    );
}

/// A filter narrows the ledger and travels with the arrow: an arrow that forgot it would answer
/// a narrowed list with an unnarrowed page, which is the thing the `custom_id` carries it for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_filter_narrows_the_screen_and_travels_with_the_arrow(pool: PgPool) {
    let money = setup_money(&pool).await;

    for _ in 0..6 {
        pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 10).await;
    }

    // One in the other currency, which the filter has to keep out of both pages. It is paid by
    // the reader rather than to them, and in a currency they hold, because that is what a
    // payment needs — what it is here for is the currency it is in.
    pay(&pool, MONEY_USER2, MONEY_USER1, &money.unit2, 999).await;

    let first = interaction(
        router(pool.clone()),
        history(MONEY_USER2, "pay", vec![option("unit", json!(&money.unit))]),
    )
    .await;

    let shown = rendered(&first);
    assert!(shown.contains("**送金の履歴** (6件)"), "{shown}");
    assert!(
        !shown.contains("**999**"),
        "the other currency is filtered out: {shown}"
    );

    let pressed = interaction(
        router(pool.clone()),
        support::button_from_guild(json!({ "custom_id": next_button(&first) }), MONEY_USER2),
    )
    .await;

    let second = rendered(&pressed);
    assert!(second.contains("**送金の履歴** (6件)"), "{second}");
    assert!(
        !second.contains("**999**"),
        "the second page is still the filtered one: {second}"
    );
}

/// The issuance ledger: what the guild's pool paid, who it paid, and how many rows there are.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issue_shows_what_the_pool_paid(pool: PgPool) {
    setup_money(&pool).await;

    issue(&pool, MONEY_USER1, 300).await;
    issue(&pool, OTHER, 200).await;

    let screen = interaction(
        router(pool.clone()),
        issue_in_guild(MONEY_USER2, support::DEFAULT_PERMISSIONS, vec![]),
    )
    .await;

    assert_eq!(screen.status, 200, "body: {}", screen.body);

    let shown = rendered(&screen);
    assert!(shown.contains("**発行の履歴** (2件)"), "{shown}");
    assert!(shown.contains("発行: **300**"), "{shown}");
    assert!(shown.contains("発行: **200**"), "{shown}");
    assert!(shown.contains(&MONEY_USER1.to_string()), "{shown}");
    assert!(shown.contains(&OTHER.to_string()), "{shown}");

    // Narrowed to one member, which is the question an administrator asks of a pool.
    let narrowed = interaction(
        router(pool.clone()),
        issue_in_guild(
            MONEY_USER2,
            support::DEFAULT_PERMISSIONS,
            vec![option("user", json!(MONEY_USER1.to_string()))],
        ),
    )
    .await;

    let shown = rendered(&narrowed);
    assert!(shown.contains("**発行の履歴** (1件)"), "{shown}");
    assert!(!shown.contains("**200**"), "{shown}");
}

/// What may be seen follows what may be done: the pool is the guild's, so the ledger of what it
/// paid is the administrator's — the same bit `/issue` asks for — and a direct message has no
/// pool to read at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issue_is_the_administrators(pool: PgPool) {
    setup_money(&pool).await;
    issue(&pool, MONEY_USER1, 300).await;

    let member = interaction(
        router(pool.clone()),
        issue_in_guild(MONEY_USER2, NO_PERMISSIONS, vec![]),
    )
    .await;
    assert!(
        rendered(&member).contains("管理者権限が必要です"),
        "{}",
        member.body
    );

    let direct = interaction(router(pool.clone()), history(MONEY_USER2, "issue", vec![])).await;
    assert!(
        rendered(&direct).contains("DMでは実行できません"),
        "{}",
        direct.body
    );

    // And the payments screen, which is nobody else's business, needs no permission at all.
    let own = interaction(
        router(pool.clone()),
        execute_from_guild(
            json!({
                "name": "history",
                "options": [{ "name": "pay", "type": 1, "options": [] }],
            }),
            MONEY_USER2,
        ),
    )
    .await;
    assert_eq!(own.status, 200, "body: {}", own.body);
    assert!(
        rendered(&own).contains("送金の履歴はありません"),
        "{}",
        own.body
    );
}

/// Nothing written yet is said rather than shown as a page of nothing, on both screens.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_ledger_says_so(pool: PgPool) {
    setup_money(&pool).await;

    let paid = interaction(router(pool.clone()), history(MONEY_USER2, "pay", vec![])).await;
    assert!(
        rendered(&paid).contains("送金の履歴はありません"),
        "{}",
        paid.body
    );

    let issued = interaction(
        router(pool.clone()),
        issue_in_guild(MONEY_USER2, support::DEFAULT_PERMISSIONS, vec![]),
    )
    .await;
    assert!(
        rendered(&issued).contains("発行の履歴はありません"),
        "{}",
        issued.body
    );
}
