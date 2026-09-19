//! `Interaction.Command.handle/4` for `/contract`, and the buttons its screen
//! carries.
//!
//! An addition with no Elixir counterpart: its contract page is a mockup whose
//! buttons carry no `phx-click`, and it has no Discord surface for contracts at
//! all. What is pinned here is the design — the screen names what the caller was
//! asked for, the buttons are the answers the contract allows, and pressing one
//! locks, refuses or returns money the way the endpoint does.

mod support;

use serde_json::json;
use sqlx::PgPool;
use support::{
    Recorded, Response, button_from_guild, execute_from_guild, fake, insert_application,
    insert_asset, insert_currency, insert_user, interaction, state_with_notifier,
};
use vc_api::custom_id::ui::contract::{Action, custom_id};

const OWNER: i32 = 1;
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;
const PARTY: i32 = 2;
const PARTY_DISCORD_ID: i64 = 100_000_000_000_000_001;
const STRANGER: i32 = 3;
const STRANGER_DISCORD_ID: i64 = 100_000_000_000_000_002;
const GUILD: i64 = 900_000_000_000_000_001;

/// An application, a user it is asking, and a balance to lock.
async fn fixture(pool: &PgPool) -> i64 {
    insert_user(pool, OWNER, OWNER_DISCORD_ID).await;
    insert_user(pool, PARTY, PARTY_DISCORD_ID).await;
    insert_user(pool, STRANGER, STRANGER_DISCORD_ID).await;
    insert_currency(pool, 1, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, PARTY, 1, 1_000).await;

    insert_application(pool, OWNER_DISCORD_ID, "an application").await
}

/// A contract asking `PARTY` for 100, made through the domain rather than the
/// endpoint: what is under test here is the screen and the buttons.
async fn asking(pool: &PgPool, application: i64, expires_in: Option<i64>) -> i64 {
    vc_core::contract::create(
        pool,
        application,
        "nyan",
        &[vc_core::contract::NewParty {
            discord_id: PARTY_DISCORD_ID,
            amount: 100,
        }],
        None,
        expires_in,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a contract")
}

fn router(pool: &PgPool, recorded: std::sync::Arc<Recorded>) -> axum::Router {
    vc_api::router(state_with_notifier(pool.clone(), fake(), recorded))
}

/// What the one container says, which is everything a screen has to say.
fn texts(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter_map(|child| child["content"].as_str().map(str::to_owned))
        .collect()
}

/// The `custom_id` of every button the screen offers, in order.
fn buttons(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter(|child| child["type"] == 1)
        .flat_map(|row| {
            row["components"]
                .as_array()
                .expect("an action row's children")
                .iter()
                .filter_map(|component| component["custom_id"].as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
        .collect()
}

async fn balance(pool: &PgPool, account: i32) -> i64 {
    sqlx::query_scalar!(
        "SELECT amount FROM assets WHERE user_id = $1 AND currency_id = 1",
        i64::from(account)
    )
    .fetch_optional(pool)
    .await
    .expect("a balance")
    .flatten()
    .unwrap_or(0)
}

/// The screen names who is asking, what the caller's part is, and how far the
/// rest has come — and offers exactly the two answers a pending contract takes.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_shows_what_the_caller_was_asked_for(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let said = texts(&response);
    assert_eq!(
        said[0],
        "**契約** (1件)\n承認すると、その分の通貨がロックされ、アプリケーションが操作できるようになります。"
    );
    assert_eq!(
        said[1],
        "**an application**（nyan） — 承認待ち\n\
         あなたの分: 100／未回答 ・ 承認 0/1 ・ 残り 0\n\
         期限: なし（いつでも取り消せます）"
    );

    assert_eq!(
        buttons(&response),
        [
            custom_id(Action::Approve, id),
            custom_id(Action::Refuse, id),
        ]
    );
}

/// A user who is named in nothing is told so, and told where such a thing would
/// appear.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_user_who_is_named_in_nothing_says_so(pool: PgPool) {
    let application = fixture(&pool).await;
    asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        execute_from_guild(
            json!({ "name": "contract", "options": [{ "name": "list", "type": 1 }] }),
            STRANGER_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
          ここに承認待ちとして並びます。"
        ]
    );
    assert!(buttons(&response).is_empty());
}

/// Pressing 承認する locks the amount, tells the application, and redraws the
/// screen as the message it came from.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_from_the_button_locks_it(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;
    let recorded = std::sync::Arc::new(Recorded::default());

    let response = interaction(
        router(&pool, recorded.clone()),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body["type"], 7,
        "the message is updated, not added to"
    );
    assert_eq!(balance(&pool, PARTY).await, 900, "locked");
    assert_eq!(
        recorded.contract_decisions(),
        [(application, id)],
        "and the application is told"
    );

    let said = texts(&response);
    assert_eq!(
        said[1],
        "**an application**（nyan） — 全員承認済み\n\
         あなたの分: 100／承認済み ・ 承認 1/1 ・ 残り 100\n\
         期限: なし（いつでも取り消せます）"
    );
    assert_eq!(
        buttons(&response),
        [custom_id(Action::Withdraw, id)],
        "and the only answer left is taking it back"
    );
}

/// Pressing 拒否する ends the contract, and what was locked comes home.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn refusing_from_the_button_ends_it(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Refuse, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        vc_core::contract::find(&pool, id)
            .await
            .expect("a read")
            .expect("the contract")
            .status,
        "canceled"
    );
    assert_eq!(
        texts(&response),
        [
            "あなたが対象になっている契約はありません。アプリケーションが契約を作ると、\
          ここに承認待ちとして並びます。"
        ],
        "a contract that is over is not shown"
    );
}

/// A button for a contract the presser is not named in is refused in a sentence.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn somebody_elses_contract_is_refused(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, None).await;

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            STRANGER_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: その契約はあなたを対象にしていません。"]
    );
    assert_eq!(balance(&pool, STRANGER).await, 0);
}

/// A temporary contract that is still running offers no 取り消す: the period is
/// what the party agreed to, and a button that would be refused is not shown.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_running_contract_offers_no_withdraw(pool: PgPool) {
    let application = fixture(&pool).await;
    let id = asking(&pool, application, Some(3_600)).await;

    let approved = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Approve, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(approved.status, 200, "body: {}", approved.body);
    assert!(buttons(&approved).is_empty(), "{:?}", buttons(&approved));

    // And asking anyway is refused the way the endpoint refuses it.
    let refused = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        button_from_guild(
            json!({ "custom_id": custom_id(Action::Withdraw, id) }),
            PARTY_DISCORD_ID,
        ),
    )
    .await;

    assert_eq!(refused.status, 200, "body: {}", refused.body);
    assert!(
        texts(&refused)[0].starts_with("エラー: その契約には今この操作ができません。"),
        "{:?}",
        texts(&refused)
    );
    assert_eq!(balance(&pool, PARTY).await, 900, "still locked");
}

/// The command runs where the user is, which is also a DM: nothing about a
/// contract is the guild's to decide.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_answers_in_a_direct_message(pool: PgPool) {
    let application = fixture(&pool).await;
    asking(&pool, application, None).await;

    let payload = json!({
        "type": 2,
        "data": { "name": "contract", "options": [{ "name": "list", "type": 1 }] },
        "user": { "id": PARTY_DISCORD_ID.to_string() },
    });

    let response = interaction(
        router(&pool, std::sync::Arc::new(Recorded::default())),
        payload,
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response).len(), 2, "the screen is the same one");
}
