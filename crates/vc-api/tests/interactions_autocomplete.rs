//! Autocomplete (`type 4`).
//!
//! The Elixir suite has no test for this, so nothing here is a port: these cases
//! follow `Interaction.AutoComplete` and the searches behind it, and pin the
//! parts that decide what a caller is offered.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    client_id_of, execute_from_guild, fake, insert_application, insert_user, interaction,
    setup_claim, state,
};

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

/// `/application show <client_id>`, and every other subcommand that names one: the uuid
/// is offered from the caller's own, so nobody types one and nobody copies one from
/// somewhere else.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_client_id_option_offers_the_callers_own_applications(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    insert_user(&pool, USER, DISCORD).await;
    let application = insert_application(&pool, DISCORD, "テスト").await;
    let client_id = client_id_of(&pool, application).await;

    let response = interaction(
        router(pool),
        autocomplete_payload(
            "application",
            focused_subcommand("show", "client_id", ""),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(8));
    assert_eq!(values(&response), vec![client_id.clone()]);

    // The label carries the name as well, because a name is not unique and the operator is
    // choosing between their own applications rather than reading a uuid.
    let label = choices(&response)[0]["name"]
        .as_str()
        .expect("a name")
        .to_owned();

    assert!(label.contains("テスト"), "{label}");
    assert!(label.contains(&client_id), "{label}");
}

/// What is typed narrows it, and Discord does no filtering of its own — it shows what it
/// is given.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_client_id_query_narrows_to_the_name_that_matches(pool: PgPool) {
    const USER: i32 = 1;
    const DISCORD: i64 = 100_000_000_000_000_001;

    insert_user(&pool, USER, DISCORD).await;
    insert_application(&pool, DISCORD, "テスト").await;
    let other = insert_application(&pool, DISCORD, "べつの").await;
    let other_id = client_id_of(&pool, other).await;

    let response = interaction(
        router(pool),
        autocomplete_payload(
            "application",
            focused_subcommand("show", "client_id", "べつ"),
            DISCORD,
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(values(&response), vec![other_id]);
}

/// `/help command:<名前>` offers every command, and they are the registered ones:
/// the suggestions and the screens are the same list read twice.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_help_option_offers_every_registered_command(pool: PgPool) {
    let response = interaction(
        router(pool),
        autocomplete_payload("help", json!([focused("command", "")]), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(8));

    let every: Vec<String> = vc_api::discord_commands::commands()
        .iter()
        .map(|command| command["name"].as_str().expect("a name").to_owned())
        .collect();

    assert_eq!(values(&response), every);
}

/// What is typed narrows it, and the order is the registration order rather than
/// alphabetical: that is the order the list and the menu show too.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_help_query_narrows_to_the_names_that_contain_it(pool: PgPool) {
    let response = interaction(
        router(pool),
        autocomplete_payload("help", json!([focused("command", "c")]), 12),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        values(&response),
        ["application", "contract", "create", "claim"]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn claim_autocomplete_names_the_application(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let application = insert_application(&pool, claims.money.user1, "Shop").await;
    let account = support::account_of(&pool, application).await;
    sqlx::query("UPDATE claims SET claimant_user_id = $1 WHERE id = $2")
        .bind(i64::from(account))
        .bind(claims.id(0))
        .execute(&pool)
        .await
        .unwrap();
    let response = interaction(
        router(pool),
        autocomplete_payload(
            "claim",
            focused_subcommand("approve", "id", &claims.id(0).to_string()),
            claims.money.user2,
        ),
    )
    .await;
    let name = choices(&response)[0]["name"].as_str().unwrap().to_owned();
    assert!(name.contains("請求元: Shop(app)"), "{name}");
}
