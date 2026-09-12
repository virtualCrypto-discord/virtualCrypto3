//! Autocomplete (`type 4`).
//!
//! The Elixir suite has no test for this, so nothing here is a port: these cases
//! follow `Interaction.AutoComplete` and the searches behind it, and pin the
//! parts that decide what a caller is offered.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{execute_from_guild, fake, interaction, setup_claim, state};

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// A `type 4` interaction: the option Discord says the user is typing in is the
/// one marked `focused`.
fn autocomplete_payload(command: &str, options: Value, user: i64) -> Value {
    let mut payload = execute_from_guild(json!({ "name": command, "options": options }), user);
    payload["type"] = json!(4);

    payload
}

fn focused(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value, "focused": true })
}

/// A subcommand's options sit one level down, and the path they make is what
/// decides which claims an `id` option offers.
fn focused_subcommand(subcommand: &str, name: &str, value: &str) -> Value {
    json!([{ "name": subcommand, "type": 1, "options": [focused(name, value)] }])
}

fn choices(response: &support::Response) -> Vec<Value> {
    response.body["data"]["choices"]
        .as_array()
        .expect("the choices")
        .clone()
}

fn values(response: &support::Response) -> Vec<String> {
    choices(response)
        .iter()
        .map(|choice| choice["value"].as_str().expect("a value").to_string())
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_unit_query_offers_what_the_caller_holds(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool),
        autocomplete_payload("info", json!([focused("unit", "")]), money.user1),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(8));
    assert_eq!(values(&response), vec![money.unit.clone()]);
    assert_eq!(
        choices(&response)[0]["name"],
        json!(format!(
            "通貨名: {} 所持量: 200000{}",
            money.name, money.unit
        ))
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_typed_unit_query_narrows_to_the_prefix(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool.clone()),
        autocomplete_payload("info", json!([focused("unit", "n")]), money.user1),
    )
    .await;

    assert_eq!(values(&response), vec![money.unit.clone()]);

    // A prefix that matches nothing offers nothing.
    let response = interaction(
        router(pool),
        autocomplete_payload("info", json!([focused("unit", "zz")]), money.user1),
    )
    .await;

    assert!(values(&response).is_empty());
}

/// Approving is the payer's move, and only a pending claim can still be
/// approved, so that is what the option offers.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_id_option_under_approve_offers_only_what_can_be_approved(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool),
        autocomplete_payload(
            "claim",
            focused_subcommand("approve", "id", ""),
            money.user1,
        ),
    )
    .await;

    // user1 pays c2, and is both sides of c6, so those are the two pending ones
    // they could approve — newest first.
    assert_eq!(
        values(&response),
        vec![claims.id(5).to_string(), claims.id(1).to_string()]
    );
    assert!(
        choices(&response)[0]["name"]
            .as_str()
            .expect("a name")
            .contains(&format!("請求id: {}", claims.id(5))),
        "the suggestion names the claim"
    );
}

/// Showing a claim accepts any of them, so every status is offered.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_id_option_under_show_offers_every_status(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let money = &claims.money;

    let response = interaction(
        router(pool),
        autocomplete_payload("claim", focused_subcommand("show", "id", ""), money.user1),
    )
    .await;

    // Newest first, and user1 is a party to all six.
    let mut expected: Vec<String> = claims.ids.iter().map(|id| id.to_string()).collect();
    expected.reverse();

    assert_eq!(values(&response), expected);
}
