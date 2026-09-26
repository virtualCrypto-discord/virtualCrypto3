mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    account_of, button_from_guild, client_id_of, execute_from_guild, fake, insert_application,
    insert_asset, insert_currency, insert_user, interaction, state,
};

const OWNER: i64 = 500_000_000_000_000_001;
const PAYER: i64 = 100_000_000_000_000_001;
const BOT: i64 = 700_000_000_000_000_001;

async fn fixture(pool: &PgPool) {
    insert_user(pool, 1, OWNER).await;
    insert_user(pool, 2, PAYER).await;
    insert_currency(pool, 1, "nyan", "nyan", 900_000_000_000_000_001, 500).await;
    insert_asset(pool, 2, 1, 1_000).await;
}

async fn asking(pool: &PgPool, name: &str) -> (i64, i32, i64, String) {
    let app = insert_application(pool, OWNER, name).await;
    let account = account_of(pool, app).await;
    let claim = vc_core::claim::create(pool, account, PAYER, "nyan", 100, None)
        .await
        .unwrap();
    (app, account, claim, client_id_of(pool, app).await)
}

async fn command(pool: &PgPool, subcommand: &str, claim: Option<i64>) -> Value {
    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        execute_from_guild(json!({"name":"claim", "options":[{
            "name":subcommand, "type":1,
            "options":claim.map(|id| vec![json!({"name":"id", "value":id.to_string()})]).unwrap_or_default(),
        }]}), PAYER),
    ).await;
    assert_eq!(response.status, 200);
    response.body
}

fn children(body: &Value) -> &[Value] {
    body["data"]["components"][0]["components"]
        .as_array()
        .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unbound_claimants_are_identifiable_before_and_after_payment(pool: PgPool) {
    fixture(&pool).await;
    let (_, account, claim, client_id) = asking(&pool, "same name").await;
    let (_, _, other_claim, other_client_id) = asking(&pool, "same name").await;
    let identity = format!("請求元: Bot未連携: `same name`\nclient_id: `{client_id}`");
    let other_identity = format!("請求元: Bot未連携: `same name`\nclient_id: `{other_client_id}`");

    for subcommand in ["list", "received"] {
        let listed = command(&pool, subcommand, None).await;
        let lines: Vec<_> = children(&listed)
            .iter()
            .filter_map(|child| child["content"].as_str())
            .collect();
        assert!(lines.iter().any(|line| line.contains(&identity)));
        assert!(lines.iter().any(|line| line.contains(&other_identity)));
        assert!(!listed.to_string().contains("<@0>"));
    }
    let other = command(&pool, "show", Some(other_claim)).await;
    assert!(
        children(&other)[0]["content"]
            .as_str()
            .unwrap()
            .contains(&other_identity)
    );
    let shown = command(&pool, "show", Some(claim)).await;
    assert!(
        children(&shown)[0]["content"]
            .as_str()
            .unwrap()
            .contains(&identity)
    );
    assert!(!shown.to_string().contains(&other_client_id));
    let row = children(&shown)
        .iter()
        .find(|child| child["type"] == 1)
        .unwrap();
    let button = &row["components"][0];
    assert_eq!(button["disabled"], false);
    let api = fake();
    let paid = support::completed_interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        api,
        button_from_guild(
            json!({"component_type":2, "custom_id":button["custom_id"]}),
            PAYER,
        ),
    )
    .await;
    assert_eq!(paid.status, 202);
    assert_eq!(paid.body["type"], 7);
    assert!(
        children(&paid.body)[1]["content"]
            .as_str()
            .unwrap()
            .contains(&identity)
    );
    let received: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = $1 AND currency_id = 1")
            .bind(i64::from(account))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(received, 100);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn bound_claimants_show_the_bot_and_unbinding_restores_the_application(pool: PgPool) {
    fixture(&pool).await;
    let (app, _, claim, client_id) = asking(&pool, "").await;
    sqlx::query("UPDATE users SET discord_id = $1 WHERE application_id = $2")
        .bind(BOT)
        .bind(app)
        .execute(&pool)
        .await
        .unwrap();
    for (subcommand, id) in [("show", Some(claim)), ("received", None)] {
        let body = command(&pool, subcommand, id).await;
        assert!(body.to_string().contains(&format!("請求元: <@{BOT}>")));
        assert!(!body.to_string().contains(&client_id));
        assert!(!body.to_string().contains(&format!("<@{OWNER}>")));
        assert_eq!(body["data"]["allowed_mentions"]["parse"], json!([]));
    }
    sqlx::query("UPDATE users SET discord_id = NULL WHERE application_id = $1")
        .bind(app)
        .execute(&pool)
        .await
        .unwrap();
    let body = command(&pool, "show", Some(claim)).await;
    assert!(
        children(&body)[0]["content"]
            .as_str()
            .unwrap()
            .contains(&format!(
                "請求元: Bot未連携: `（名前なし）`\nclient_id: `{client_id}`"
            ))
    );
}
