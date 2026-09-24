mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    account_of, client_id_of, fake, insert_application, insert_user, interaction, mint_app, state,
};
use tower::ServiceExt;
use vc_api::custom_id::ui::developer::{Screen, custom_id_for_field};
use vc_api::routes::oauth2_clients::{Registration, changes, details, validated};
use vc_core::application::TextField;

const OWNER: i64 = 500_000_000_000_000_001;
const FIELDS: [TextField; 5] = [
    TextField::ClientName,
    TextField::ClientUri,
    TextField::LogoUri,
    TextField::WebhookUrl,
    TextField::SupportInviteSlug,
];

fn value(field: TextField, length: usize) -> String {
    let prefix = match field {
        TextField::ClientName => "",
        TextField::SupportInviteSlug => "a",
        _ => "https://example.com/",
    };
    // Multibyte characters must not consume several characters of the budget.
    format!("{prefix}{}", "あ".repeat(length - prefix.chars().count()))
}

fn registration(
    body: Value,
) -> Result<vc_core::application::NewApplication, Box<vc_api::routes::oauth2_clients::Refusal>> {
    validated(serde_json::from_value::<Registration>(body).unwrap())
}

#[test]
fn registration_and_updates_accept_the_limit_and_reject_one_more_character() {
    for field in FIELDS {
        for length in [field.max_chars(), field.max_chars() + 1] {
            let body = json!({field.name():value(field, length), "redirect_uris":[]});
            let registered = registration(body.clone());
            let changed = changes(body.as_object().unwrap());
            if length == field.max_chars() {
                assert!(registered.is_ok(), "{field:?}: {registered:?}");
                assert!(changed.is_ok(), "{field:?}: {changed:?}");
            } else {
                for error in [registered.unwrap_err(), changed.unwrap_err()] {
                    assert_eq!(error.status, 400);
                    assert_eq!(error.error, "invalid_client_metadata");
                    assert_eq!(
                        error.description.unwrap(),
                        format!(
                            "{}_must_be_at_most_{}_characters",
                            field.name(),
                            field.max_chars()
                        )
                    );
                }
            }
        }
    }
}

fn text_chars(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            let own = if map.get("type").and_then(Value::as_u64) == Some(10) {
                map["content"].as_str().unwrap().chars().count()
            } else {
                0
            };
            own + map.values().map(text_chars).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(text_chars).sum(),
        _ => 0,
    }
}

#[test]
fn all_settings_at_their_limits_fit_in_one_message_without_truncation() {
    // Many short URIs maximize the extra spaces added by the screen's comma
    // separators (the input budget counts one newline between URIs).
    let mut redirects = vec!["http:".to_owned(); 166];
    redirects[0].push_str("abcde");
    assert_eq!(redirects.join("\n").chars().count(), 1000);
    let mut body = json!({
        "redirect_uris":redirects, "application_type":"native",
        "grant_types":["authorization_code", "refresh_token"],
        "response_types":["code"], "subscribed_events":[2,3,4]
    });
    for field in FIELDS {
        body[field.name()] = json!(value(field, field.max_chars()));
    }
    let new = registration(body).unwrap();
    // Reserve even the DB's full secret column and a 512-character connection
    // URL; the generated secret is only 64 characters.
    let secret = "s".repeat(255);
    let token = format!("https://example.com/{}", "t".repeat(492));
    for connected in [false, true] {
        let screen = vc_api::developer::application(
            "00000000-0000-0000-0000-000000000000",
            new.client_name.as_deref(),
            connected,
            new.logo_uri.as_deref(),
            Some(&secret),
            (!connected).then_some(token.as_str()),
            vc_api::developer::Fields {
                client_name: new.client_name.as_deref(),
                redirect_uris: &new.redirect_uris,
                client_uri: new.client_uri.as_deref(),
                logo_uri: new.logo_uri.as_deref(),
                webhook_url: new.webhook_url.as_deref(),
                discord_support_server_invite_slug: new
                    .discord_support_server_invite_slug
                    .as_deref(),
                application_type: &new.application_type,
                grant_types: &new.grant_types,
                response_types: &new.response_types,
                subscribed_events: &new.subscribed_events,
            },
        );
        let count = text_chars(&screen);
        assert!(count <= 4000, "settings use {count} characters");
        eprintln!("maximum settings screen: connected={connected}, {count} characters");
        let serialized = screen.to_string();
        for field in FIELDS {
            assert!(serialized.contains(&value(field, field.max_chars())));
        }
    }
}

async fn patch(pool: &PgPool, token: &str, body: Value) -> (u16, Value) {
    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri("/oauth2/clients/@me")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn oversized_updates_are_client_errors_and_leave_all_settings_unchanged(pool: PgPool) {
    let app = insert_application(&pool, OWNER, "before").await;
    let token = mint_app(&pool, account_of(&pool, app).await, &["oauth2.register"]).await;
    let before = details(&pool, app).await.unwrap();
    for field in FIELDS {
        let (status, body) = patch(
            &pool,
            &token,
            json!({
                field.name():value(field, field.max_chars() + 1), "client_secret":true,
                "redirect_uris":["https://example.com/new"]
            }),
        )
        .await;
        assert_eq!(status, 400, "{field:?}: {body}");
        assert_eq!(body["error"], "invalid_client_metadata");
        assert_eq!(details(&pool, app).await.unwrap(), before);
    }
    assert_eq!(
        patch(
            &pool,
            &token,
            json!({"client_name":value(TextField::ClientName, 100)})
        )
        .await
        .0,
        204
    );
    assert_eq!(
        details(&pool, app).await.unwrap().unwrap().client_name,
        Some(value(TextField::ClientName, 100))
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn repeated_menu_values_are_saved_once(pool: PgPool) {
    let app = insert_application(&pool, OWNER, "app").await;
    let token = mint_app(&pool, account_of(&pool, app).await, &["oauth2.register"]).await;
    assert_eq!(
        patch(
            &pool,
            &token,
            json!({
                "grant_types":vec!["authorization_code"; 1000],
                "response_types":vec!["code"; 1000], "subscribed_events":vec![2;1000]
            })
        )
        .await
        .0,
        204
    );
    let saved = details(&pool, app).await.unwrap().unwrap();
    assert_eq!(saved.grant_types, ["authorization_code"]);
    assert_eq!(saved.response_types, ["code"]);
    assert_eq!(saved.subscribed_events, [2]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn discord_inputs_share_the_limits_and_overlong_submissions_are_explained(pool: PgPool) {
    insert_user(&pool, 1, OWNER).await;
    let app = insert_application(&pool, OWNER, "before").await;
    let client_id = client_id_of(&pool, app).await;
    let fields = FIELDS.map(|field| (field.name(), field.max_chars()));
    for (field, limit) in fields.into_iter().chain([("redirect_uris", 1000)]) {
        let response = interaction(vc_api::router(state(pool.clone(), fake())), json!({
            "type":3, "user":{"id":OWNER.to_string()},
            "data":{"component_type":2, "custom_id":custom_id_for_field(Screen::Edit, &client_id, field)}
        })).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.body["type"], 9);
        assert_eq!(
            response.body["data"]["components"][0]["component"]["max_length"],
            limit
        );
    }
    let response = interaction(
        vc_api::router(state(pool.clone(), fake())),
        json!({
            "type":5, "user":{"id":OWNER.to_string()},
            "data":{"custom_id":custom_id_for_field(Screen::Edit, &client_id, "client_name"),
                "components":[{"type":18,"component":{
                    "type":4,"custom_id":"client_name","value":"a".repeat(101)
                }}]}
        }),
    )
    .await;
    assert_eq!(response.status, 200);
    assert!(
        response
            .body
            .to_string()
            .contains("client_name_must_be_at_most_100_characters")
    );
    assert_eq!(
        details(&pool, app)
            .await
            .unwrap()
            .unwrap()
            .client_name
            .as_deref(),
        Some("before")
    );
}
