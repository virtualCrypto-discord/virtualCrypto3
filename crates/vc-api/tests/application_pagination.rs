mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    account_of, execute_from_dm, fake, insert_application, insert_user, interaction, mint_app,
    state,
};
use tower::ServiceExt;

const USER: i64 = 100_000_000_000_000_001;

fn options(body: &Value) -> &Vec<Value> {
    body["data"]["components"][0]["components"][1]["components"][0]["options"]
        .as_array()
        .unwrap()
}
fn arrows(body: &Value) -> &Vec<Value> {
    body["data"]["components"][0]["components"][2]["components"]
        .as_array()
        .unwrap()
}
async fn press(pool: &PgPool, body: &Value, arrow: usize) -> Value {
    let id = arrows(body)[arrow]["custom_id"].as_str().unwrap();
    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        json!({
            "type":3, "user":{"id":USER.to_string()},
            "data":{"component_type":2,"custom_id":id}
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body["type"], 7);
    response.body
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn all_applications_are_reachable_through_the_four_arrows(pool: PgPool) {
    insert_user(&pool, 1, USER).await;
    for index in 0..51 {
        insert_application(&pool, USER, &format!("app{index:02}")).await;
    }
    insert_application(&pool, USER + 1, "somebody else's").await;
    let first = interaction(
        vc_api::router(state(pool.clone(), fake())),
        execute_from_dm(
            json!({"name":"application","options":[{"name":"list","type":1}]}),
            USER,
        ),
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(options(&first.body).len(), 25);
    assert_eq!(arrows(&first.body)[0]["disabled"], true);
    assert_eq!(arrows(&first.body)[1]["disabled"], true);
    let second = press(&pool, &first.body, 2).await;
    assert_eq!(options(&second).len(), 25);
    assert_eq!(options(&second)[0]["label"], "app25");
    let last = press(&pool, &first.body, 3).await;
    assert_eq!(options(&last).len(), 1);
    assert_eq!(options(&last)[0]["label"], "app50");
    assert_eq!(arrows(&last)[2]["disabled"], true);
    assert_eq!(arrows(&last)[3]["disabled"], true);
    assert_eq!(options(&press(&pool, &last, 1).await), options(&second));
    assert_eq!(options(&press(&pool, &last, 0).await), options(&first.body));
    assert_eq!(options(&press(&pool, &second, 2).await), options(&last));
    let menu = &last["data"]["components"][0]["components"][1]["components"][0];
    let selected = options(&last)[0]["value"].as_str().unwrap();
    let shown = interaction(
        vc_api::router(state(pool.clone(), fake())),
        json!({
            "type":3,"user":{"id":USER.to_string()},
            "data":{"component_type":3,"custom_id":menu["custom_id"],"values":[selected]}
        }),
    )
    .await;
    assert_eq!(shown.status, 200);
    assert_eq!(shown.body["type"], 7);
    assert!(
        shown.body["data"]["components"][0]["components"][0]["content"]
            .as_str()
            .unwrap()
            .contains("app50")
    );

    // A previously valid last-page button remains usable after the list shrinks.
    sqlx::query("UPDATE applications SET owner_discord_id = $1 WHERE client_name = 'app50'")
        .bind(USER + 1)
        .execute(&pool)
        .await
        .unwrap();
    let shrunk = press(&pool, &first.body, 3).await;
    assert_eq!(options(&shrunk).len(), 25);
    assert_eq!(options(&shrunk)[0]["label"], "app25");
    assert_eq!(arrows(&shrunk)[3]["disabled"], true);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_empty_name_saved_through_the_api_still_has_a_valid_menu_label(pool: PgPool) {
    insert_user(&pool, 1, USER).await;
    let application = insert_application(&pool, USER, "before").await;
    let account = account_of(&pool, application).await;
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri("/oauth2/clients/@me")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            json!({"client_name":""}).to_string(),
        ))
        .unwrap();
    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 204);

    // `interaction` also validates the entire response against Discord's schema.
    let response = interaction(
        vc_api::router(state(pool, fake())),
        execute_from_dm(
            json!({"name":"application","options":[{"name":"list","type":1}]}),
            USER,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(options(&response.body)[0]["label"], "（名前なし）");
}

#[test]
fn pagination_starts_only_after_twenty_five_applications() {
    for count in [0, 1, 25, 26] {
        let apps: Vec<_> = (0..count)
            .map(|i| {
                json!({
                    "client_id":format!("00000000-0000-4000-8000-{i:012}"),
                    "client_name":format!("app{i}")
                })
            })
            .collect();
        let screen = vc_api::developer::applications(&apps, &Default::default());
        let children = screen["components"].as_array().unwrap();
        if count == 0 {
            assert_eq!(children.len(), 1);
        } else {
            assert_eq!(
                children[1]["components"][0]["options"]
                    .as_array()
                    .unwrap()
                    .len(),
                count.min(25)
            );
            assert_eq!(children.len(), if count > 25 { 3 } else { 2 });
        }
    }
}
