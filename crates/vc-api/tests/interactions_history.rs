//! `/history`: the two ledgers, read back.
//!
//! An addition rather than a port — the Elixir writes both tables and reads neither, and no
//! command of its names a history — so nothing here has an Elixir case behind it:
//! `docs/known-gaps.md` is the design, and this file holds it to the two things that file
//! claims. What may be seen follows what may be done, and what a row says is what happened:
//! a payment names both sides, and a contract's lock and return name the contract beside the one
//! side of the wallet that moved. A charge credits the recipient, but does not
//! debit the payer's wallet a second time after the lock.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    MONEY_GUILD, MONEY_USER1, MONEY_USER2, Response, execute_from_dm, execute_from_guild, fake,
    from_guild, insert_application, insert_user, interaction, setup_money, state,
};
use tower::ServiceExt;

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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pagination_preserves_an_imported_emoji_unit(pool: PgPool) {
    let money = setup_money(&pool).await;
    // This format exists in the production v2 data, despite v3's stricter
    // validation for newly created currencies.
    let unit = "<:winvista_calcexe:870168179052253215>";
    sqlx::query("UPDATE currencies SET unit = $1 WHERE id = $2")
        .bind(unit)
        .bind(money.currency)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..6 {
        pay(&pool, MONEY_USER1, MONEY_USER2, unit, 10).await;
    }
    pay(&pool, MONEY_USER2, MONEY_USER1, "w", 1).await;

    let app = router(pool);
    for other in [None, Some(MONEY_USER1)] {
        let mut options = vec![option("unit", json!(unit))];
        if let Some(other) = other {
            options.push(option("user", json!(other.to_string())));
        }
        let first = interaction(app.clone(), history(MONEY_USER2, "pay", options)).await;
        assert_eq!(first.status, 200, "{}", first.body);
        assert!(rendered(&first).contains("(6件)"), "{}", first.body);

        let second = interaction(
            app.clone(),
            support::button_from_guild(json!({"custom_id": next_button(&first)}), MONEY_USER2),
        )
        .await;
        assert_eq!(second.status, 200, "{}", second.body);
        let shown = rendered(&second);
        assert!(shown.contains("(6件)"), "{shown}");
        assert!(shown.contains(unit), "{shown}");
        assert_eq!(shown.matches("受取: **10**").count(), 1, "{shown}");
        assert!(!shown.contains("`w`"), "{shown}");
    }
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

/// A contract's money is the wallet's money when it moves. Approving locks it and the screen
/// says so; what the application then spends from the lock is the escrow's movement, not the
/// wallet's, and is not in this ledger; and the end of the contract sends the remainder home as
/// a return. The charge that happened in between is nowhere in the count, which is the whole of
/// what the predicate is for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_contract_lock_and_return_are_in_the_wallet_history(pool: PgPool) {
    let money = setup_money(&pool).await;

    let application = insert_application(&pool, MONEY_USER1, "a metered service").await;
    let now = time::OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        &money.unit,
        &[vc_core::contract::NewParty {
            discord_id: MONEY_USER1,
            amount: 100,
        }],
        None,
        None,
        now,
    )
    .await
    .expect("a contract");

    // The approval locks the money, so it is a movement and it shows.
    vc_core::contract::approve(&pool, contract, 1, || now)
        .await
        .expect("an approval");

    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(screen.contains("**送金の履歴** (1件)"), "{screen}");
    assert!(
        screen.contains("契約にロック: **100** `n` → Bot未連携: `a metered service`"),
        "{screen}"
    );

    // What the application spends from the lock moves the escrow, not the wallet, so it is not in
    // this ledger at all.
    vc_core::contract::pay(&pool, contract, application, OTHER, None, 25, || now)
        .await
        .expect("a charge");

    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(
        screen.contains("**送金の履歴** (1件)"),
        "a charge is not the wallet's: {screen}"
    );
    assert!(!screen.contains("**25**"), "{screen}");

    let receiver =
        rendered(&interaction(router(pool.clone()), history(OTHER, "pay", vec![])).await);
    assert!(receiver.contains("**送金の履歴** (1件)"), "{receiver}");
    assert!(
        receiver.contains("契約から受取: **25** `n` ← Bot未連携: `a metered service`"),
        "{receiver}"
    );

    // The end of the contract returns what is left of the lock, and that is a movement.
    vc_core::contract::withdraw(&pool, contract, 1, || now)
        .await
        .expect("a withdrawal");

    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(screen.contains("**送金の履歴** (2件)"), "{screen}");
    assert!(
        screen.contains("契約から返却: **75** `n` ← Bot未連携: `a metered service`"),
        "{screen}"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn edited_application_names_cannot_forge_contract_history(pool: PgPool) {
    const BOT: i64 = 500_000_000_000_000_003;
    let money = setup_money(&pool).await;
    let application = insert_application(&pool, MONEY_USER1, "actual service").await;
    let account = support::account_of(&pool, application).await;
    let client_id = support::client_id_of(&pool, application).await;
    let token = support::mint_app(&pool, account, &["oauth2.register"]).await;
    let now = time::OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        &money.unit,
        &[vc_core::contract::NewParty {
            discord_id: MONEY_USER2,
            amount: 10,
        }],
        None,
        None,
        now,
    )
    .await
    .unwrap();
    vc_core::contract::approve(&pool, contract, 2, || now)
        .await
        .unwrap();
    vc_core::contract::pay(&pool, contract, application, OTHER, None, 3, || now)
        .await
        .unwrap();
    vc_core::contract::withdraw(&pool, contract, 2, || now)
        .await
        .unwrap();

    // Change the name after the movements happened, through the real edit API.
    let app = router(pool.clone());
    let forged_name = "`\n<@500000000000000004>\r\n受取: **999999** @everyone <@&123>";
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("PATCH")
                .uri("/oauth2/clients/@me")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    json!({"client_name": forged_name}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 204);

    for bound in [false, true] {
        let identity = if bound {
            vc_core::user::bind_bot(&pool, account, BOT).await.unwrap();
            format!("<@{BOT}>")
        } else {
            format!(
                "Bot未連携: `｀ <@500000000000000004>  受取: **999999** @everyone <@&123>`\nclient_id: `{client_id}`"
            )
        };
        // Lock, return, and incoming charge all use the same safe identity.
        for (reader, prefixes) in [
            (
                MONEY_USER2,
                vec!["契約から返却: **7** `n` ← ", "契約にロック: **10** `n` → "],
            ),
            (OTHER, vec!["契約から受取: **3** `n` ← "]),
        ] {
            let response = interaction(app.clone(), history(reader, "pay", vec![])).await;
            assert_eq!(response.status, 200);
            assert_eq!(
                response.body["data"]["allowed_mentions"]["parse"],
                json!([])
            );
            let rows: Vec<_> = response.body["data"]["components"][0]["components"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|value| value["content"].as_str())
                .skip(1)
                .collect();
            assert_eq!(rows.len(), prefixes.len());
            for (row, prefix) in rows.into_iter().zip(prefixes) {
                assert!(
                    row.starts_with(&format!("{prefix}{identity} ・ <t:")),
                    "{row}"
                );
                assert!(!row.contains(forged_name), "{row}");
                assert_eq!(row.matches('\n').count(), usize::from(!bound), "{row}");
                if bound {
                    assert!(!row.contains("999999"), "{row}");
                    assert!(!row.contains(&client_id), "{row}");
                }
            }
        }
    }
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

/// An issuance to the reader is money arriving in their wallet, so it is on their own ledger's
/// screen too: merged with the payments in one list, counted with them, and read as money coming
/// in — the pool rather than a person on the other side of the arrow.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_issuance_to_the_reader_is_on_their_own_screen(pool: PgPool) {
    let money = setup_money(&pool).await;

    issue(&pool, MONEY_USER2, 300).await;
    pay(&pool, MONEY_USER1, MONEY_USER2, &money.unit, 500).await;

    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER2, "pay", vec![])).await);

    // Both ledgers, and the count is the merged list's rather than one table's.
    assert!(screen.contains("**送金の履歴** (2件)"), "{screen}");
    assert!(screen.contains("発行: **300** `n` ← 発行枠"), "{screen}");
    assert!(screen.contains("受取: **500**"), "{screen}");

    // And it is personal: what was issued to somebody else is not on this screen.
    let other =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(!other.contains("発行: **300**"), "{other}");
}

/// A payment whose receiver is one of the contract's parties puts the money back in that party's
/// wallet, and their own ledger shows it: a return, not a charge the wallet's history hides.
///
/// Two parties and no fixed receiver, so the draw is the oldest approval first and the money
/// lands in one of their wallets — one movement of the escrow into one wallet, which is why the
/// ledger gets one row for it rather than a slice per party drawn on.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_payment_to_a_party_is_a_return_in_the_wallet_history(pool: PgPool) {
    let money = setup_money(&pool).await;

    let application = insert_application(&pool, MONEY_USER1, "a metered service").await;
    let now = time::OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        &money.unit,
        &[
            vc_core::contract::NewParty {
                discord_id: MONEY_USER1,
                amount: 100,
            },
            vc_core::contract::NewParty {
                discord_id: MONEY_USER2,
                amount: 60,
            },
        ],
        None,
        None,
        now,
    )
    .await
    .expect("a contract");

    vc_core::contract::approve(&pool, contract, 1, || now)
        .await
        .expect("an approval");
    vc_core::contract::approve(&pool, contract, 2, || now)
        .await
        .expect("an approval");

    // 120 out of the escrow into the first party's wallet: her own hundred and twenty of the
    // second party's.
    let payed =
        vc_core::contract::pay(&pool, contract, application, MONEY_USER1, None, 120, || now)
            .await
            .expect("a return");

    assert_eq!(payed.amount, 120);

    // The wallet's ledger has the lock and the return, and not a charge.
    let screen =
        rendered(&interaction(router(pool.clone()), history(MONEY_USER1, "pay", vec![])).await);
    assert!(screen.contains("**送金の履歴** (2件)"), "{screen}");
    assert!(
        screen.contains("契約にロック: **100** `n` → Bot未連携: `a metered service`"),
        "{screen}"
    );
    assert!(
        screen.contains("契約から返却: **120** `n` ← Bot未連携: `a metered service`"),
        "{screen}"
    );

    // And the contract's own statement agrees, in one row: the escrow sent it, and the party
    // received it.
    let statement =
        vc_core::contract::payments(&pool, contract, vc_core::page::Cursor::First, None)
            .await
            .expect("a statement");

    assert_eq!(statement[0].event, "return");
    assert_eq!(statement[0].amount, 120);
    assert_eq!(statement[0].discord_id, None, "the escrow sent it");
    assert_eq!(statement[0].receiver_discord_id, Some(MONEY_USER1));
}
