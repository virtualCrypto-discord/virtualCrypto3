//! The `bal` command, ported from
//! `test/virtualCrypto_web/controllers/api/interactions/bal_test.exs`.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{execute_from_guild, fake, interaction, setup_money, state};

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

fn bal_data() -> Value {
    json!({ "name": "bal", "options": [] })
}

/// What the one container says, which is everything this screen has to say.
fn texts(response: &Value) -> Vec<String> {
    response["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter_map(|child| child["content"].as_str().map(str::to_owned))
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_lists_the_currency_the_user_holds(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), money.user1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(4));
    assert_eq!(
        texts(&response.body),
        [
            "**残高一覧** (1件)".to_string(),
            format!("**{}**\n**199,500** `{}`", money.name, money.unit),
        ]
    );
    assert_eq!(
        response.body["data"]["components"][0]["accent_color"],
        json!(0x6221ED),
    );

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}

/// The list is ordered by unit, which for these two currencies is `n` then `w`.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_lists_every_currency_in_unit_order(pool: PgPool) {
    let money = setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), money.user2)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response.body),
        [
            "**残高一覧** (2件)".to_string(),
            format!("**{}**\n**1,000** `{}`", money.name, money.unit),
            format!("**{}**\n**200,000** `{}`", money.name2, money.unit2),
        ]
    );

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}

/// A discord id with no account has no assets to join, so the command answers
/// with the empty list rather than creating anything.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_for_an_unknown_user_says_there_is_nothing(pool: PgPool) {
    setup_money(&pool).await;

    let response = interaction(router(pool), execute_from_guild(bal_data(), -1)).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response.body), ["表示できる通貨はありません。"]);

    assert_eq!(response.body["data"]["flags"], json!(32832),);
}

/// Eleven currencies, which is one more than a page holds: the screen says what the caller
/// holds altogether, the arrows reach the rest, and the second page has one row with the way
/// back.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bal_pages_when_there_are_more_currencies_than_a_screen_holds(pool: PgPool) {
    const USER: i64 = 100_000_000_000_000_003;

    support::insert_user(&pool, 3, USER).await;

    for index in 0..11 {
        let id = i64::from(index) + 10;
        let unit = ((b'a' + index as u8) as char).to_string();

        support::insert_currency(&pool, id, &unit, &unit, 900_000_000_000_000_000 + id, 0).await;
        support::insert_asset(&pool, 3, id, 100).await;
    }

    let first = interaction(router(pool.clone()), execute_from_guild(bal_data(), USER)).await;

    assert_eq!(first.status, 200, "body: {}", first.body);
    assert_eq!(texts(&first.body)[0], "**残高一覧** (11件)");
    assert_eq!(
        texts(&first.body).len(),
        11,
        "the header and the page's ten: {}",
        first.body
    );

    let arrows = buttons(&first.body);

    assert_eq!(arrows.len(), 4);
    assert_eq!(arrows[0], "disabled-0");
    assert_eq!(arrows[1], "disabled-1");
    assert_eq!(
        arrows[2],
        vc_api::custom_id::ui::bal::page_custom_id(vc_api::custom_id::ui::bal::Page::Next, 2)
    );
    assert_eq!(
        arrows[3],
        vc_api::custom_id::ui::bal::page_custom_id(vc_api::custom_id::ui::bal::Page::Last, 2)
    );

    let second = interaction(
        router(pool.clone()),
        support::button_from_guild(json!({ "custom_id": arrows[2] }), USER),
    )
    .await;

    assert_eq!(second.status, 200, "body: {}", second.body);
    assert_eq!(second.body["type"], 7, "a redraw: {}", second.body);
    assert_eq!(
        texts(&second.body),
        [
            "**残高一覧** (11件)".to_string(),
            "**k**\n**100** `k`".to_string(),
        ]
    );

    let arrows = buttons(&second.body);

    assert_eq!(
        arrows[0],
        vc_api::custom_id::ui::bal::page_custom_id(vc_api::custom_id::ui::bal::Page::First, 1)
    );
    assert_eq!(
        arrows[1],
        vc_api::custom_id::ui::bal::page_custom_id(vc_api::custom_id::ui::bal::Page::Previous, 1)
    );
    assert_eq!(arrows[2], "disabled-2");
    assert_eq!(arrows[3], "disabled-3");

    // Every arrow, pressed: the four are one call with a different number in the id, and
    // the number is the page the screen that comes back shows.
    for (arrow, rows) in [(0, 10), (1, 10)] {
        let back = interaction(
            router(pool.clone()),
            support::button_from_guild(json!({ "custom_id": arrows[arrow] }), USER),
        )
        .await;

        assert_eq!(back.status, 200, "body: {}", back.body);
        assert_eq!(back.body["type"], 7, "a redraw: {}", back.body);
        assert_eq!(texts(&back.body).len(), rows + 1, "the header and the page");
        assert_eq!(texts(&back.body)[0], "**残高一覧** (11件)");
    }

    // And ⏩ from the first screen: the last page, which is the same one ⏭️ reached.
    let last = interaction(
        router(pool),
        support::button_from_guild(json!({ "custom_id": buttons(&first.body)[3] }), USER),
    )
    .await;

    assert_eq!(last.status, 200, "body: {}", last.body);
    assert_eq!(
        texts(&last.body),
        [
            "**残高一覧** (11件)".to_string(),
            "**k**\n**100** `k`".to_string(),
        ]
    );
}

/// The pagination row's `custom_id`s, in order.
fn buttons(response: &Value) -> Vec<String> {
    response["data"]["components"][0]["components"]
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
