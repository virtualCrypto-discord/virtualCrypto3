mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{fake, interaction, state};
use vc_api::custom_id::ui::developer::{Screen, custom_id_for, custom_id_for_field};
use vc_core::application::{self, TokenEndpointAuthMethod as Method};

const OWNER: i64 = 500_000_000_000_000_001;

fn components(value: &Value) -> Vec<&Value> {
    let mut found = vec![];
    match value {
        Value::Object(map) => {
            if map.contains_key("custom_id") {
                found.push(value);
            }
            for child in map.values() {
                found.extend(components(child));
            }
        }
        Value::Array(children) => {
            for child in children {
                found.extend(components(child));
            }
        }
        _ => {}
    }
    found
}

async fn press(pool: &PgPool, actor: i64, custom_id: &str, values: Option<Value>) -> Value {
    let mut data = json!({"component_type":2,"custom_id":custom_id});
    if let Some(values) = values {
        data["component_type"] = json!(3);
        data["values"] = values;
    }
    let api = fake();
    let response = interaction(
        vc_api::router(state(pool.clone(), api.clone())),
        support::callback_fields(json!({"type":3,"user":{"id":actor.to_string()},"data":data})),
    )
    .await;
    if response.status == 202 {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            api.response_edit_finished(),
        )
        .await
        .expect("settings response delivered");
        assert_eq!(api.callbacks(), [json!({"type":6})]);
        return json!({"type":7,"data":api.response_edits().pop().unwrap()});
    }
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.body["type"], 7);
    response.body
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn owner_can_open_change_and_reopen_auth_settings(pool: PgPool) {
    support::insert_user(&pool, 1, OWNER).await;
    let app = support::insert_application(&pool, OWNER, "auth settings").await;
    let client = support::client_id_of(&pool, app).await;
    let shown = press(&pool, OWNER, &custom_id_for(Screen::Show, &client), None).await;
    let open = custom_id_for(Screen::TokenAuth, &client);
    assert!(
        components(&shown)
            .iter()
            .any(|component| component["custom_id"] == open)
    );
    assert!(shown.to_string().contains("クライアント認証なし"));
    for (value, expected) in [
        ("client_secret_basic", Method::ClientSecretBasic),
        ("none", Method::None),
    ] {
        let screen = press(&pool, OWNER, &open, None).await;
        let menu_id = custom_id_for_field(Screen::Edit, &client, "token_endpoint_auth_method");
        let menu = components(&screen)
            .into_iter()
            .find(|c| c["custom_id"] == menu_id)
            .unwrap();
        assert_eq!(menu["min_values"], 1);
        assert_eq!(menu["max_values"], 1);
        assert!(
            components(&screen)
                .iter()
                .any(|c| c["custom_id"] == custom_id_for(Screen::RotateSecret, &client))
        );
        assert!(
            components(&screen)
                .iter()
                .any(|c| c["custom_id"] == custom_id_for(Screen::Show, &client))
        );
        let updated = press(&pool, OWNER, &menu_id, Some(json!([value]))).await;
        assert_eq!(
            application::token_endpoint_auth_method(&pool, app)
                .await
                .unwrap(),
            expected
        );
        assert!(updated.to_string().contains(if expected == Method::None {
            "クライアント認証なし"
        } else {
            "Basic 認証必須"
        }));
        let reopened = press(&pool, OWNER, &open, None).await;
        let menu = components(&reopened)
            .into_iter()
            .find(|c| c["custom_id"] == menu_id)
            .unwrap();
        let defaults: Vec<_> = menu["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["default"] == true)
            .collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0]["value"], value);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn non_owner_cannot_read_or_change_auth_and_invalid_method_is_refused(pool: PgPool) {
    support::insert_user(&pool, 1, OWNER).await;
    let app = support::insert_application(&pool, OWNER, "auth settings").await;
    sqlx::query("UPDATE applications SET client_secret='private-test-secret' WHERE id=$1")
        .bind(app)
        .execute(&pool)
        .await
        .unwrap();
    let client = support::client_id_of(&pool, app).await;
    let denied = press(
        &pool,
        OWNER + 1,
        &custom_id_for(Screen::TokenAuth, &client),
        None,
    )
    .await;
    assert!(!denied.to_string().contains("private-test-secret"));
    let menu = custom_id_for_field(Screen::Edit, &client, "token_endpoint_auth_method");
    press(
        &pool,
        OWNER + 1,
        &menu,
        Some(json!(["client_secret_basic"])),
    )
    .await;
    assert_eq!(
        application::token_endpoint_auth_method(&pool, app)
            .await
            .unwrap(),
        Method::None
    );
    let invalid = press(&pool, OWNER, &menu, Some(json!(["unknown"]))).await;
    assert!(
        invalid
            .to_string()
            .contains("invalid_token_endpoint_auth_method")
    );
    assert_eq!(
        application::token_endpoint_auth_method(&pool, app)
            .await
            .unwrap(),
        Method::None
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn owner_can_open_change_and_reopen_pkce_settings(pool: PgPool) {
    support::insert_user(&pool, 1, OWNER).await;
    let app = support::insert_application(&pool, OWNER, "auth settings").await;
    let client = support::client_id_of(&pool, app).await;
    let shown = press(&pool, OWNER, &custom_id_for(Screen::Show, &client), None).await;
    let open = custom_id_for(Screen::TokenAuth, &client);
    assert!(
        components(&shown)
            .iter()
            .any(|component| component["custom_id"] == open)
    );
    assert!(
        shown
            .to_string()
            .contains("任意（サーバーの必須化期限まで）")
    );
    for (value, expected) in [
        ("required", application::PkcePolicy::Required),
        ("optional", application::PkcePolicy::Optional),
    ] {
        let screen = press(&pool, OWNER, &open, None).await;
        let menu_id = custom_id_for_field(Screen::Edit, &client, "pkce_policy");
        let menu = components(&screen)
            .into_iter()
            .find(|c| c["custom_id"] == menu_id)
            .unwrap();
        assert_eq!(menu["min_values"], 1);
        assert_eq!(menu["max_values"], 1);
        assert!(
            components(&screen)
                .iter()
                .any(|c| c["custom_id"] == custom_id_for(Screen::RotateSecret, &client))
        );
        assert!(
            components(&screen)
                .iter()
                .any(|c| c["custom_id"] == custom_id_for(Screen::Show, &client))
        );
        let updated = press(&pool, OWNER, &menu_id, Some(json!([value]))).await;
        assert_eq!(
            application::pkce_policy(&pool, app).await.unwrap(),
            expected
        );
        assert!(
            updated
                .to_string()
                .contains(if expected == application::PkcePolicy::Optional {
                    "任意（サーバーの必須化期限まで）"
                } else {
                    "必須（S256）"
                })
        );
        let reopened = press(&pool, OWNER, &open, None).await;
        let menu = components(&reopened)
            .into_iter()
            .find(|c| c["custom_id"] == menu_id)
            .unwrap();
        let defaults: Vec<_> = menu["options"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["default"] == true)
            .collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0]["value"], value);
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn non_owner_cannot_change_pkce_and_invalid_policy_is_refused(pool: PgPool) {
    support::insert_user(&pool, 1, OWNER).await;
    let app = support::insert_application(&pool, OWNER, "auth settings").await;
    sqlx::query("UPDATE applications SET client_secret='private-test-secret' WHERE id=$1")
        .bind(app)
        .execute(&pool)
        .await
        .unwrap();
    let client = support::client_id_of(&pool, app).await;
    let denied = press(
        &pool,
        OWNER + 1,
        &custom_id_for(Screen::TokenAuth, &client),
        None,
    )
    .await;
    assert!(!denied.to_string().contains("private-test-secret"));
    let menu = custom_id_for_field(Screen::Edit, &client, "pkce_policy");
    press(&pool, OWNER + 1, &menu, Some(json!(["required"]))).await;
    assert_eq!(
        application::pkce_policy(&pool, app).await.unwrap(),
        application::PkcePolicy::Optional
    );
    let invalid = press(&pool, OWNER, &menu, Some(json!(["unknown"]))).await;
    assert!(invalid.to_string().contains("invalid_pkce_policy"));
    assert_eq!(
        application::pkce_policy(&pool, app).await.unwrap(),
        application::PkcePolicy::Optional
    );
}
