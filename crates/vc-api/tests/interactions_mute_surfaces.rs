//! Mutes filter each reader's lists, counts, cursors and suggestions, not money or details.
mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::*;
use time::OffsetDateTime;
use vc_core::mute::{self, Target};

const OTHER: i64 = 100_000_000_000_000_003;
const BOT: i64 = 100_000_000_000_000_004;

async fn screen(app: Router, user: i64, name: &str, options: Value) -> Response {
    let mut payload = execute_from_guild(json!({"name":name,"options":options}), user);
    payload["guild_id"] = json!(MONEY_GUILD.to_string());
    let response = interaction(app, payload).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response
}

async fn history(app: Router, user: i64, kind: &str, options: Value) -> Response {
    screen(
        app,
        user,
        "history",
        json!([{"name":kind,"type":1,"options":options}]),
    )
    .await
}

fn count(response: &Response, count: usize) {
    assert!(
        response.body.to_string().contains(&format!("全{count}件")),
        "{}",
        response.body
    );
}

fn forward(response: &Response) -> String {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["components"].as_array())
        .flatten()
        .find(|button| button["disabled"] == false)
        .unwrap()["custom_id"]
        .as_str()
        .unwrap()
        .into()
}

fn currency(money: &Money) -> Target {
    Target::Currency {
        id: money.currency,
        unit: money.unit.clone(),
        name: money.name.clone(),
    }
}

async fn read(app: Router, path: &str, token: &str) -> Vec<Value> {
    let response = get(app, path, Some(token)).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.body.as_array().unwrap().clone()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn balances_hide_muted_currencies_without_falsifying_details_or_blocking_payments(
    pool: PgPool,
) {
    let money = setup_money(&pool).await;
    for id in 3..=12 {
        insert_currency(
            &pool,
            id,
            &format!("Currency{id}"),
            &format!("x{id}"),
            id,
            0,
        )
        .await;
        insert_asset(&pool, 2, id, id).await;
    }
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let token = mint(&pool, 2, &[]).await;
    let before = screen(app.clone(), money.user2, "bal", json!([])).await;
    assert!(before.body.to_string().contains("(12件)"));
    let stale_next = forward(&before);
    for unit in [&money.unit, &money.unit2] {
        mute::mute_currency(&pool, 2, unit, OffsetDateTime::now_utc())
            .await
            .unwrap();
    }
    let after = screen(app.clone(), money.user2, "bal", json!([])).await;
    assert!(after.body.to_string().contains("(10件)"));
    assert!(!after.body.to_string().contains("nyan"));
    assert!(!after.body.to_string().contains("wan"));
    let holdings = read(app.clone(), "/api/v2/users/@me/balances", &token).await;
    assert_eq!(holdings.len(), 10);
    assert!(
        holdings
            .iter()
            .all(|row| row["currency"]["unit"].as_str().unwrap().starts_with('x'))
    );
    let stale = interaction(
        app.clone(),
        button_from_guild(json!({"custom_id":stale_next}), money.user2),
    )
    .await;
    assert!(stale.body.to_string().contains("(10件)"));
    assert!(
        stale
            .body
            .to_string()
            .contains("このページには何もありません")
    );

    // Explicit detail and payment still use the real amount, even when the list is hidden.
    let info = screen(
        app.clone(),
        money.user2,
        "info",
        json!([{"name":"unit","value":"n"}]),
    )
    .await;
    assert!(
        info.body.to_string().contains("**1,000** `n`"),
        "{}",
        info.body
    );
    let paid = screen(
        app.clone(),
        money.user2,
        "pay",
        json!([
            {"name":"unit","value":"n"}, {"name":"user","value":money.user1.to_string()},
            {"name":"amount","value":1}
        ]),
    )
    .await;
    assert_eq!(
        paid.body["data"]["flags"].as_i64().unwrap_or(0) & 64,
        0,
        "{}",
        paid.body
    );
    let actual: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id=2 AND currency_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actual, 999);
    let other = screen(app.clone(), money.user1, "bal", json!([])).await;
    assert!(other.body.to_string().contains("nyan"));

    mute_ui::remove(discord, app.clone(), money.user2, &currency(&money)).await;
    let restored = read(app, "/api/v2/users/@me/balances", &token).await;
    assert_eq!(restored.len(), 11);
    assert!(
        restored
            .iter()
            .any(|row| row["currency"]["unit"] == "n" && row["amount"] == "999")
    );
    assert!(restored.iter().all(|row| row["currency"]["unit"] != "w"));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn payment_history_filters_both_ledgers_counts_and_api_cursors_then_restores_each_mute(
    pool: PgPool,
) {
    let money = setup_money(&pool).await;
    insert_user(&pool, 3, OTHER).await;
    insert_asset(&pool, 3, money.currency2, 100).await;
    for _ in 0..6 {
        vc_core::payment::pay_from_discord(&pool, OTHER, money.user2, "w", 1)
            .await
            .unwrap();
    }
    vc_core::payment::pay_from_discord(&pool, money.user1, money.user2, "n", 7)
        .await
        .unwrap();
    vc_core::payment::pay_from_discord(&pool, money.user2, money.user1, "n", 8)
        .await
        .unwrap();
    vc_core::issue::issue(&pool, money.guild, money.user2, Some(9))
        .await
        .unwrap();
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let token = mint(&pool, 2, &[]).await;
    let original = read(app.clone(), "/api/v2/users/@me/transactions", &token).await;
    assert_eq!(original.len(), 9);
    mute::mute_currency(&pool, 2, "n", OffsetDateTime::now_utc())
        .await
        .unwrap();
    let first = history(app.clone(), money.user2, "pay", json!([])).await;
    count(&first, 6);
    assert!(!first.body.to_string().contains("`n`"));
    let last = interaction(
        app.clone(),
        button_from_guild(json!({"custom_id":forward(&first)}), money.user2),
    )
    .await;
    count(&last, 6);
    assert!(last.body.to_string().contains("6–6件"), "{}", last.body);

    let mut path = "/api/v2/users/@me/transactions?limit=2".to_string();
    let mut visible = Vec::new();
    for _ in 0..4 {
        let response = get(app.clone(), &path, Some(&token)).await;
        assert_eq!(response.status, 200);
        visible.extend(response.body.as_array().unwrap().iter().cloned());
        let Some(link) = response.headers.get("link") else {
            break;
        };
        let query = link
            .to_str()
            .unwrap()
            .split_once('?')
            .unwrap()
            .1
            .split('>')
            .next()
            .unwrap();
        path = format!("/api/v2/users/@me/transactions?{query}");
    }
    assert_eq!(visible.len(), 6);
    assert!(visible.iter().all(|row| row["unit"] == "w"));
    let mut ids: Vec<_> = visible.iter().map(|row| row["id"].clone()).collect();
    ids.sort_by_key(Value::to_string);
    ids.dedup();
    assert_eq!(ids.len(), 6);
    mute::mute_user(&pool, 2, OTHER, OffsetDateTime::now_utc())
        .await
        .unwrap();
    count(
        &history(app.clone(), money.user2, "pay", json!([])).await,
        0,
    );
    count(
        &history(
            app.clone(),
            money.user2,
            "pay",
            json!([{"name":"unit","value":"n"}]),
        )
        .await,
        0,
    );
    assert!(
        read(app.clone(), "/api/v2/users/@me/transactions?unit=n", &token)
            .await
            .is_empty()
    );
    assert!(
        read(
            app.clone(),
            &format!("/api/v2/users/@me/transactions?related_discord_user_id={OTHER}"),
            &token
        )
        .await
        .is_empty()
    );
    let other_token = mint(&pool, 1, &[]).await;
    assert_eq!(
        read(app.clone(), "/api/v2/users/@me/transactions", &other_token)
            .await
            .len(),
        2
    );
    mute_ui::remove(discord.clone(), app.clone(), money.user2, &currency(&money)).await;
    let restored = read(app.clone(), "/api/v2/users/@me/transactions", &token).await;
    assert_eq!(restored.len(), 3);
    assert!(
        restored
            .iter()
            .any(|row| row["event"] == "issue" && row["balance_after"] == "1008")
    );
    mute_ui::remove(
        discord,
        app.clone(),
        money.user2,
        &Target::User { discord_id: OTHER },
    )
    .await;
    assert_eq!(
        read(app, "/api/v2/users/@me/transactions", &token).await,
        original
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn issuance_screen_filters_its_viewer_but_the_guild_api_has_no_personal_mutes(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, 3, OTHER).await;
    for receiver in [
        money.user2,
        OTHER,
        money.user2,
        OTHER,
        money.user2,
        money.user2,
    ] {
        vc_core::issue::issue(&pool, money.guild, receiver, Some(1))
            .await
            .unwrap();
    }
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    count(
        &history(app.clone(), money.user1, "issue", json!([])).await,
        6,
    );
    mute::mute_user(&pool, 1, money.user2, OffsetDateTime::now_utc())
        .await
        .unwrap();
    count(
        &history(app.clone(), money.user1, "issue", json!([])).await,
        2,
    );
    count(
        &history(
            app.clone(),
            money.user1,
            "issue",
            json!([{"name":"user","value":money.user2.to_string()}]),
        )
        .await,
        0,
    );
    mute::mute_currency(&pool, 1, "n", OffsetDateTime::now_utc())
        .await
        .unwrap();
    count(
        &history(app.clone(), money.user1, "issue", json!([])).await,
        0,
    );
    count(&history(app.clone(), OTHER, "issue", json!([])).await, 6);
    let application = insert_application(&pool, money.user1, "issuance reader").await;
    let token = insert_grant(&pool, application, money.guild, &["vc.issue"]).await;
    assert_eq!(
        read(app.clone(), "/api/v2/currencies/1/issuances", &token)
            .await
            .len(),
        6
    );
    mute_ui::remove(discord.clone(), app.clone(), money.user1, &currency(&money)).await;
    count(
        &history(app.clone(), money.user1, "issue", json!([])).await,
        2,
    );
    mute_ui::remove(
        discord,
        app.clone(),
        money.user1,
        &Target::User {
            discord_id: money.user2,
        },
    )
    .await;
    count(&history(app, money.user1, "issue", json!([])).await, 6);
}

async fn claim_choices(app: Router, user: i64, subcommand: &str, query: &str) -> Vec<String> {
    let mut payload = execute_from_dm(
        json!({"name":"claim","options":[{
            "name":subcommand,"type":1,"options":[{"name":"id","value":query,"focused":true}]
        }]}),
        user,
    );
    payload["type"] = json!(4);
    let response = interaction(app, payload).await;
    assert_eq!(response.status, 200, "{}", response.body);
    response.body["data"]["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| choice["value"].as_str().unwrap().into())
        .collect()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn claim_suggestions_apply_currency_and_user_mutes_to_every_subcommand(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, 3, OTHER).await;
    for (id, claimant, payer, currency) in [(1, 1, 2, 1), (2, 3, 2, 2), (3, 2, 1, 2), (4, 2, 3, 2)]
    {
        insert_claim(&pool, id, 1, "pending", claimant, payer, currency).await;
    }
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    assert_eq!(
        claim_choices(app.clone(), money.user2, "show", "")
            .await
            .len(),
        4
    );
    mute::mute_currency(&pool, 2, "n", OffsetDateTime::now_utc())
        .await
        .unwrap();
    mute::mute_user(&pool, 2, OTHER, OffsetDateTime::now_utc())
        .await
        .unwrap();
    for subcommand in ["show", "approve", "deny", "cancel"] {
        let expected: Vec<String> = if ["show", "cancel"].contains(&subcommand) {
            vec!["3".into()]
        } else {
            vec![]
        };
        assert_eq!(
            claim_choices(app.clone(), money.user2, subcommand, "").await,
            expected
        );
        for query in ["1", "2", "4"] {
            assert!(
                claim_choices(app.clone(), money.user2, subcommand, query)
                    .await
                    .is_empty()
            );
        }
    }
    let limited = vc_core::claim::search_candidates(
        &pool,
        money.user2,
        "",
        vc_core::claim::SrFilter::All,
        &["pending".to_string()],
        None,
        1,
    )
    .await
    .unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].id, 3);
    assert_eq!(
        claim_choices(app.clone(), money.user1, "show", "")
            .await
            .len(),
        2
    );
    mute_ui::remove(
        discord.clone(),
        app.clone(),
        money.user2,
        &Target::User { discord_id: OTHER },
    )
    .await;
    assert_eq!(
        claim_choices(app.clone(), money.user2, "show", "")
            .await
            .len(),
        3
    );
    mute_ui::remove(discord, app.clone(), money.user2, &currency(&money)).await;
    assert_eq!(claim_choices(app, money.user2, "show", "").await.len(), 4);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_relationship_mutes_hide_lists_locks_refunds_and_charges(pool: PgPool) {
    let money = setup_money(&pool).await;
    insert_user(&pool, 3, OTHER).await;
    let application = insert_application(&pool, money.user1, "merchant").await;
    let application_account = account_of(&pool, application).await;
    vc_core::user::bind_bot(&pool, application_account, BOT)
        .await
        .unwrap();
    let now = OffsetDateTime::now_utc();
    let contract = vc_core::contract::create(
        &pool,
        application,
        "n",
        &[
            vc_core::contract::NewParty {
                discord_id: money.user1,
                amount: 10,
            },
            vc_core::contract::NewParty {
                discord_id: money.user2,
                amount: 10,
            },
        ],
        Some(OTHER),
        None,
        now,
    )
    .await
    .unwrap();
    for account in [1, 2] {
        vc_core::contract::approve(&pool, contract, account, || now)
            .await
            .unwrap();
    }
    vc_core::contract::pay(
        &pool,
        contract,
        application,
        OTHER,
        Some(money.user2),
        3,
        || now,
    )
    .await
    .unwrap();
    vc_core::contract::pay(
        &pool,
        contract,
        application,
        money.user2,
        Some(money.user2),
        2,
        || now,
    )
    .await
    .unwrap();
    let discord = fake();
    let app = vc_api::router(state(pool.clone(), discord.clone()));
    let token = mint(&pool, 2, &[]).await;
    let merchant = mint(&pool, 3, &[]).await;
    let original = read(app.clone(), "/api/v2/users/@me/transactions", &token).await;
    assert_eq!(original.len(), 2);
    assert!(original.iter().any(|row| row["event"] == "lock"));
    assert!(original.iter().any(|row| row["event"] == "return"));
    for target in [BOT, OTHER, money.user1] {
        mute::mute_user(&pool, 2, target, now).await.unwrap();
        let contracts = screen(
            app.clone(),
            money.user2,
            "contract",
            json!([{"name":"list","type":1}]),
        )
        .await;
        assert!(
            !contracts.body.to_string().contains("merchant"),
            "{}",
            contracts.body
        );
        assert!(
            read(app.clone(), "/api/v2/users/@me/contracts", &token)
                .await
                .is_empty()
        );
        count(
            &history(app.clone(), money.user2, "pay", json!([])).await,
            0,
        );
        assert!(
            read(app.clone(), "/api/v2/users/@me/transactions", &token)
                .await
                .is_empty()
        );
        let page = vc_core::contract::open_of_party(&pool, money.user2, 1, 5)
            .await
            .unwrap();
        assert_eq!(page.total, 0);
        assert!(page.next.is_none());
        // Explicit contract detail is still readable, and another reader keeps the charge.
        assert_eq!(
            get(
                app.clone(),
                &format!("/api/v2/contracts/{contract}"),
                Some(&token)
            )
            .await
            .status,
            200
        );
        assert_eq!(
            read(app.clone(), "/api/v2/users/@me/transactions", &merchant)
                .await
                .len(),
            1
        );
        mute_ui::remove(
            discord.clone(),
            app.clone(),
            money.user2,
            &Target::User { discord_id: target },
        )
        .await;
        assert_eq!(
            read(app.clone(), "/api/v2/users/@me/contracts", &token)
                .await
                .len(),
            1
        );
        assert_eq!(
            read(app.clone(), "/api/v2/users/@me/transactions", &token).await,
            original
        );
    }
    mute::mute_user(&pool, 3, BOT, now).await.unwrap();
    count(&history(app.clone(), OTHER, "pay", json!([])).await, 0);
    assert!(
        read(app.clone(), "/api/v2/users/@me/transactions", &merchant)
            .await
            .is_empty()
    );
    // Currency mute covers all contract movements too.
    mute::mute_currency(&pool, 2, "n", now).await.unwrap();
    assert!(
        read(app, "/api/v2/users/@me/transactions", &token)
            .await
            .is_empty()
    );
}

#[test]
fn unmute_is_absent_from_command_registration_and_help() {
    assert!(
        !vc_api::discord_commands::commands()
            .iter()
            .any(|command| command["name"] == "unmute")
    );
    assert!(
        !vc_api::docs::showings()
            .iter()
            .any(|command| command.name == "unmute")
    );
}
