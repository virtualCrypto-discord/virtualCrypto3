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
    account_of, client_id_of, execute_from_guild, fake, insert_application, insert_asset,
    insert_claim, insert_currency, insert_user, interaction, setup_claim, setup_money, state,
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

#[test]
fn grant_code_is_registered_for_autocomplete() {
    let commands = vc_api::discord_commands::commands();
    let grant = commands
        .iter()
        .find(|command| command["name"] == "grant")
        .unwrap();
    let approve = grant["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["name"] == "approve")
        .unwrap();
    let code = approve["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["name"] == "code")
        .unwrap();
    assert_eq!(code["autocomplete"], true);
    assert_eq!(code["type"], 3);
    assert_eq!(code["required"], true);
}

async fn grant_request(
    pool: &PgPool,
    application: i64,
    target: vc_core::grant::Target,
) -> vc_core::grant::GrantRequest {
    let scope = match target {
        vc_core::grant::Target::Guild(_) => "vc.issue",
        vc_core::grant::Target::User(_) => "vc.delegate.profile.read",
    };
    vc_core::grant::request_grant(
        pool,
        application,
        target,
        &[scope.to_owned()],
        &[],
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn grant_codes_only_offer_live_requests_the_actor_can_review(pool: PgPool) {
    use vc_core::grant::Target;
    const ME: i64 = 123;
    let guild = support::DEFAULT_GUILD;
    let application = insert_application(&pool, 900_001, "申請元").await;
    let other_app = insert_application(&pool, 900_002, "決定済み・期限切れ").await;
    let server = grant_request(&pool, application, Target::Guild(guild)).await;
    let other_server = grant_request(&pool, application, Target::Guild(guild + 1)).await;
    let personal = grant_request(&pool, application, Target::User(ME)).await;
    let other_person = grant_request(&pool, application, Target::User(ME + 1)).await;
    let approved = grant_request(&pool, other_app, Target::Guild(guild)).await;
    let now = time::OffsetDateTime::now_utc();
    let review = vc_core::grant::review_requests(
        &pool,
        Some(&approved.user_code),
        None,
        ME,
        Some(guild),
        now,
    )
    .await
    .unwrap();
    vc_core::grant::approve_review(&pool, &review[0], now)
        .await
        .unwrap()
        .unwrap();
    let expired = grant_request(&pool, other_app, Target::Guild(guild)).await;
    sqlx::query(
        "UPDATE grant_requests SET inserted_at = inserted_at - interval '1 hour' WHERE id = $1",
    )
    .bind(expired.id)
    .execute(&pool)
    .await
    .unwrap();

    for (location, permissions, actor, mut expected) in [
        (
            Some(guild),
            Some("8"),
            ME,
            vec![server.user_code.clone(), personal.user_code.clone()],
        ),
        (
            Some(guild + 1),
            Some("8"),
            ME,
            vec![other_server.user_code.clone(), personal.user_code.clone()],
        ),
        (Some(guild), Some("0"), ME, vec![personal.user_code.clone()]),
        (Some(guild), None, ME, vec![personal.user_code.clone()]),
        (None, None, ME, vec![personal.user_code.clone()]),
        (None, None, ME + 1, vec![other_person.user_code.clone()]),
        (None, None, ME + 2, vec![]),
    ] {
        let mut payload =
            autocomplete_payload("grant", focused_subcommand("approve", "code", ""), actor);
        match location {
            Some(id) => {
                payload["guild_id"] = json!(id.to_string());
                payload["member"]["permissions"] = json!(permissions);
            }
            None => {
                payload["user"] = payload["member"]["user"].clone();
                payload.as_object_mut().unwrap().remove("member");
                payload.as_object_mut().unwrap().remove("guild_id");
            }
        }
        let response = interaction(router(pool.clone()), payload).await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(response.body["type"], 8);
        let mut actual = values(&response);
        actual.sort();
        expected.sort();
        assert_eq!(
            actual, expected,
            "guild={location:?}, permissions={permissions:?}, actor={actor}"
        );
    }

    // Choosing a code still opens the review; it does not grant access.
    let command = execute_from_guild(
        json!({"name": "grant", "options": [{
            "name": "approve", "type": 1,
            "options": [{"name": "code", "type": 3, "value": server.user_code}]
        }]}),
        ME,
    );
    let review = interaction(router(pool.clone()), command.clone()).await;
    assert_eq!(review.status, 200, "{}", review.body);
    assert!(
        review
            .body
            .to_string()
            .contains("アプリケーションからの申請")
    );
    assert!(review.body.to_string().contains("申請元"));
    assert!(review.body.to_string().contains("承認する"));

    // A code offered while authorized cannot be used after losing that permission.
    let mut denied = command;
    denied["member"]["permissions"] = json!("0");
    let denied = interaction(router(pool.clone()), denied).await;
    assert_eq!(denied.status, 200);
    assert!(!denied.body.to_string().contains("承認する"));
    let state: (String, Option<i64>) =
        sqlx::query_as("SELECT status, grant_id FROM grant_requests WHERE id = $1")
            .bind(server.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, ("pending".to_owned(), None));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn grant_code_prefixes_filter_before_the_limit_and_keep_labels_readable(pool: PgPool) {
    let guild = support::DEFAULT_GUILD;
    let mut requests = Vec::new();
    for index in 0..27 {
        let name = if index == 0 {
            String::new()
        } else {
            format!("\n{}\r", "長い名前".repeat(20))
        };
        let application = insert_application(&pool, 910_000 + index, &name).await;
        let request = grant_request(&pool, application, vc_core::grant::Target::Guild(guild)).await;
        let code = if index == 0 {
            "ab120001".to_owned()
        } else {
            format!("c{index:07}")
        };
        sqlx::query("UPDATE grant_requests SET user_code = $1 WHERE id = $2")
            .bind(&code)
            .bind(request.id)
            .execute(&pool)
            .await
            .unwrap();
        requests.push((code, client_id_of(&pool, application).await));
        if index == 26 {
            let account = account_of(&pool, application).await;
            vc_core::user::bind_bot(&pool, account, 920_001)
                .await
                .unwrap();
        }
    }

    let response = interaction(
        router(pool.clone()),
        autocomplete_payload("grant", focused_subcommand("approve", "code", ""), 123),
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(choices(&response).len(), 25);
    let expected = requests
        .iter()
        .rev()
        .take(25)
        .map(|(code, _)| code.clone())
        .collect::<Vec<_>>();
    assert_eq!(values(&response), expected);
    assert!(
        choices(&response)[0]["name"]
            .as_str()
            .unwrap()
            .contains("Bot 920001")
    );
    for (choice, (_, client_id)) in choices(&response)
        .iter()
        .skip(1)
        .zip(requests.iter().rev().skip(1))
    {
        let label = choice["name"].as_str().unwrap();
        assert_eq!(label.chars().count(), 100);
        assert!(label.starts_with(choice["value"].as_str().unwrap()));
        assert!(label.contains("サーバー / Bot 未接続:"));
        assert!(label.contains(client_id));
        assert!(!label.contains(['\r', '\n']));
    }

    for (prefix, expected) in [
        ("ab12", vec!["ab120001"]),
        (" AB12 ", vec!["ab120001"]),
        ("ab120001", vec!["ab120001"]),
        ("1200", vec![]),
        ("missing", vec![]),
        ("%", vec![]),
        ("_", vec![]),
    ] {
        let response = interaction(
            router(pool.clone()),
            autocomplete_payload("grant", focused_subcommand("approve", "code", prefix), 123),
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(values(&response), expected, "prefix={prefix}");
        if !expected.is_empty() {
            let label = choices(&response)[0]["name"].as_str().unwrap().to_owned();
            assert!(label.contains("（名前なし）"));
            assert!(label.contains(&requests[0].1));
        }
    }
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
        json!(format!("通貨名: {} 残高: 200000{}", money.name, money.unit))
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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pay_unit_choices_follow_the_focused_option_in_guilds_and_dms(pool: PgPool) {
    let money = setup_money(&pool).await;

    for in_guild in [true, false] {
        for (query, expected) in [
            ("", vec![money.unit.clone()]),
            ("N", vec![money.unit.clone()]),
            ("w", vec![money.unit2.clone()]),
            ("missing", vec![]),
        ] {
            let mut payload = autocomplete_payload(
                "pay",
                json!([
                    {"name": "user", "type": 6, "value": money.user2.to_string()},
                    {"name": "amount", "type": 4, "value": 1},
                    {"name": "unit", "type": 3, "value": query, "focused": true},
                ]),
                money.user1,
            );
            if !in_guild {
                payload["user"] = payload["member"]["user"].clone();
                payload.as_object_mut().unwrap().remove("member");
                payload.as_object_mut().unwrap().remove("guild_id");
            }
            let response = interaction(router(pool.clone()), payload).await;

            assert_eq!(response.status, 200, "body: {}", response.body);
            assert_eq!(response.body["type"], 8);
            assert_eq!(
                values(&response),
                expected,
                "guild={in_guild}, query={query}"
            );
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pay_offers_the_guild_currency_to_a_user_without_holdings(pool: PgPool) {
    let money = setup_money(&pool).await;
    let mut payload = autocomplete_payload("pay", json!([focused("unit", "")]), 123);
    payload["guild_id"] = json!(money.guild.to_string());

    let response = interaction(router(pool), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(values(&response), [money.unit.as_str()]);
    assert_eq!(
        choices(&response)[0]["name"],
        format!("通貨名: {} 残高: 0{}", money.name, money.unit),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pay_limits_unit_choices_and_prioritizes_the_guild_currency(pool: PgPool) {
    let money = setup_money(&pool).await;
    for id in 3..=28 {
        let unit = format!("n{}", char::from(b'a' + (id - 3) as u8));
        insert_currency(&pool, id, &format!("Currency{id}"), &unit, id, 0).await;
        insert_asset(&pool, 1, id, 1).await;
    }

    for query in ["", "n"] {
        let mut payload = autocomplete_payload("pay", json!([focused("unit", query)]), money.user1);
        payload["guild_id"] = json!("25");
        let response = interaction(router(pool.clone()), payload).await;

        assert_eq!(response.status, 200, "body: {}", response.body);
        assert_eq!(choices(&response).len(), 25);
        assert_eq!(values(&response)[0], "nw");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn currency_mutes_hide_pay_choices_and_unmuting_restores_them(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_currency(&pool, 3, "nyanko", "na", 3, 0).await;
    insert_asset(&pool, 2, 3, 10).await;
    let discord = fake();
    let app = vc_api::router(state(pool, discord.clone()));

    // Keep a second mute when restoring n, so removal cannot accidentally clear both.
    for (changes, visible) in [
        (vec![], vec!["n", "na", "w"]),
        (vec![("mute", "n"), ("mute", "na")], vec!["w"]),
        (vec![("remove", "n")], vec!["n", "w"]),
        (vec![("remove", "na")], vec!["n", "na", "w"]),
    ] {
        for (command, unit) in changes {
            if command == "remove" {
                support::mute_ui::remove(
                    discord.clone(),
                    app.clone(),
                    money.user2,
                    &vc_core::mute::Target::Currency {
                        id: if unit == "n" { money.currency } else { 3 },
                        unit: unit.into(),
                        name: String::new(),
                    },
                )
                .await;
                continue;
            }
            let response = support::rendered_interaction(
                discord.clone(),
                app.clone(),
                support::execute_from_dm(
                    json!({
                        "name": command,
                        "options": [{"name": "currency", "type": 1,
                            "options": [{"name": "unit", "value": unit}]}],
                    }),
                    money.user2,
                ),
            )
            .await;
            assert_eq!(response.status, 202, "{}", response.body);
            assert!(
                response.body.to_string().contains("ミュートしました"),
                "{}",
                response.body
            );
        }

        for in_guild in [true, false] {
            for query in ["", "N", "na", "w"] {
                let mut payload =
                    autocomplete_payload("pay", json!([focused("unit", query)]), money.user2);
                payload["guild_id"] = json!(money.guild.to_string());
                if !in_guild {
                    payload["user"] = payload["member"]["user"].clone();
                    payload.as_object_mut().unwrap().remove("member");
                    payload.as_object_mut().unwrap().remove("guild_id");
                }
                let response = interaction(app.clone(), payload).await;
                assert_eq!(response.status, 200, "{}", response.body);
                assert_eq!(response.body["type"], 8);
                let mut actual = values(&response);
                actual.sort();
                let expected: Vec<_> = visible
                    .iter()
                    .filter(|unit| unit.starts_with(&query.to_lowercase()))
                    .copied()
                    .collect();
                assert_eq!(actual, expected, "guild={in_guild}, query={query}");
            }
        }

        // The same guild and prefix still offer both currencies to another caller.
        let mut payload = autocomplete_payload("pay", json!([focused("unit", "n")]), money.user1);
        payload["guild_id"] = json!(money.guild.to_string());
        let response = interaction(app.clone(), payload).await;
        assert_eq!(values(&response), ["n", "na"]);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn currency_mutes_apply_to_name_choices(pool: PgPool) {
    let money = setup_money(&pool).await;
    vc_core::mute::mute_currency(&pool, 2, &money.unit, time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    let app = router(pool);

    for (query, expected) in [("", vec!["wan"]), ("NY", vec![])] {
        let response = interaction(
            app.clone(),
            autocomplete_payload("info", json!([focused("name", query)]), money.user2),
        )
        .await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(values(&response), expected);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pay_filters_mutes_before_limiting_choices(pool: PgPool) {
    let money = setup_money(&pool).await;
    for id in 3..=28 {
        let unit = format!("n{}", char::from(b'a' + (id - 3) as u8));
        insert_currency(&pool, id, &format!("Currency{id}"), &unit, id, 0).await;
        insert_asset(&pool, 1, id, 1).await;
    }
    for unit in ["n", "nw"] {
        vc_core::mute::mute_currency(&pool, 1, unit, time::OffsetDateTime::now_utc())
            .await
            .unwrap();
    }

    for query in ["", "n"] {
        let mut payload = autocomplete_payload("pay", json!([focused("unit", query)]), money.user1);
        payload["guild_id"] = json!("25");
        let response = interaction(router(pool.clone()), payload).await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(choices(&response).len(), 25);
        assert!(
            !values(&response)
                .iter()
                .any(|unit| unit == "n" || unit == "nw")
        );
    }
}

/// Older currency names can exceed today's creation limit. One long label must
/// not make Discord reject every suggestion, and the selected unit stays exact.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn long_currency_names_fit_choices_without_changing_the_selected_unit(pool: PgPool) {
    let money = setup_money(&pool).await;
    for (id, name, unit) in [(3, "a".repeat(100), "na"), (4, "猫".repeat(100), "nb")] {
        insert_currency(&pool, id, &name, unit, id, 0).await;
        insert_asset(&pool, 1, id, 100).await;
    }

    for query in ["", "n"] {
        let response = interaction(
            router(pool.clone()),
            autocomplete_payload("pay", json!([focused("unit", query)]), money.user1),
        )
        .await;

        assert_eq!(response.status, 200, "body: {}", response.body);
        let mut units = values(&response);
        units.sort();
        assert_eq!(units, ["n", "na", "nb"]);
        for choice in choices(&response) {
            let length = choice["name"].as_str().unwrap().chars().count();
            assert!((1..=100).contains(&length));
            if choice["value"] != money.unit {
                assert_eq!(length, 100);
                assert!(
                    choice["name"]
                        .as_str()
                        .unwrap()
                        .ends_with(&format!(" 残高: 100{}", choice["value"].as_str().unwrap()))
                );
            }
        }
    }

    // The same renderer serves /info name; shortening its label must not
    // change the currency name submitted when the user selects it.
    let response = interaction(
        router(pool),
        autocomplete_payload("info", json!([focused("name", "猫")]), money.user1),
    )
    .await;
    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(values(&response), ["猫".repeat(100)]);
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
            .contains(&format!("請求ID: {}", claims.id(5))),
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

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn long_application_names_fit_claim_choices_without_changing_the_ids(pool: PgPool) {
    let claims = setup_claim(&pool).await;
    let payer = claims.money.user1;
    for (id, name) in [(100, "a".repeat(100)), (101, "猫".repeat(100))] {
        let app = insert_application(&pool, payer, &name).await;
        let account = account_of(&pool, app).await;
        insert_claim(&pool, id, 100, "pending", account, 1, claims.money.currency).await;
    }

    for command in ["approve", "deny", "show"] {
        // The interaction helper validates every choice against Discord's schema.
        let response = interaction(
            router(pool.clone()),
            autocomplete_payload("claim", focused_subcommand(command, "id", "10"), payer),
        )
        .await;
        assert_eq!(response.status, 200);
        assert_eq!(values(&response), ["101", "100"]);
        for choice in choices(&response) {
            let name = choice["name"].as_str().unwrap();
            assert_eq!(name.chars().count(), 100);
            assert!(name.contains(&format!("請求ID: {}", choice["value"].as_str().unwrap())));
        }
    }
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
