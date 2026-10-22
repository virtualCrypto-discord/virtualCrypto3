mod support;

use axum::{Router, body::Body, http::Request};
use base64::Engine;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use vc_api::routes::oauth2_clients::{self, Registration};
use vc_core::application::{self, TokenEndpointAuthMethod as Method};

const REDIRECT: &str = "https://app.example/callback";

async fn fixture(pool: &PgPool) -> (i64, String, String) {
    let id = support::insert_application(pool, support::MONEY_USER1, "policy").await;
    sqlx::query(
        "UPDATE applications SET client_secret = 'secret',
        grant_types = ARRAY['authorization_code','refresh_token']::openid_connect_grant_types[]
        WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
        VALUES ($1, $2, now(), now())",
    )
    .bind(id)
    .bind(REDIRECT)
    .execute(pool)
    .await
    .unwrap();
    let client: uuid::Uuid = sqlx::query_scalar("SELECT client_id FROM applications WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    let client = client.to_string();
    let code = application::authorize(
        pool,
        42,
        &[],
        &[],
        REDIRECT,
        &client,
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    (id, client, code)
}

async fn set_method(pool: &PgPool, id: i64, method: Method) {
    application::patch(
        pool,
        id,
        &application::Changes {
            token_endpoint_auth_method: Some(method),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

async fn token(app: &Router, body: Value, credentials: Option<(&str, &str)>) -> (u16, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/oauth2/token")
        .header("content-type", "application/json");
    if let Some((id, secret)) = credentials {
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{id}:{secret}"));
        req = req.header("authorization", format!("Basic {basic}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    if status == 401 {
        assert_eq!(
            response.headers()["www-authenticate"],
            "Basic realm=\"oauth2/token\""
        );
    }
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn code_and_refresh_enforce_owner_policy_without_consuming_on_failure(pool: PgPool) {
    let (id, client, code) = fixture(&pool).await;
    let (_, other, _) = fixture(&pool).await;
    set_method(&pool, id, Method::ClientSecretBasic).await;
    let app = vc_api::router(support::state(pool.clone(), support::fake()));
    let mut request = json!({"grant_type":"authorization_code","client_id":client,
        "redirect_uri":REDIRECT,"code":code});
    for credentials in [
        None,
        Some((client.as_str(), "wrong")),
        Some((other.as_str(), "secret")),
    ] {
        let (_, body) = token(&app, request.clone(), credentials).await;
        assert_eq!(body["error"], "invalid_client");
    }
    request["client_id"] = json!(other);
    assert_eq!(
        token(&app, request.clone(), Some((&client, "secret")))
            .await
            .1["error"],
        "invalid_client"
    );
    request.as_object_mut().unwrap().remove("client_id");
    let (status, issued) = token(&app, request.clone(), Some((&client, "secret"))).await;
    assert_eq!(status, 200, "{issued}");
    // An unauthenticated replay must not revoke the successfully issued grant.
    request["client_id"] = json!(client);
    assert_eq!(
        token(&app, request, None).await.1["error"],
        "invalid_client"
    );

    let mut refresh =
        json!({"grant_type":"refresh_token", "refresh_token":issued["refresh_token"]});
    for credentials in [
        None,
        Some((client.as_str(), "wrong")),
        Some((other.as_str(), "secret")),
    ] {
        assert_eq!(
            token(&app, refresh.clone(), credentials).await.1["error"],
            "invalid_client"
        );
    }
    refresh["client_id"] = json!(other);
    assert_eq!(
        token(&app, refresh.clone(), Some((&client, "secret")))
            .await
            .1["error"],
        "invalid_client"
    );
    refresh.as_object_mut().unwrap().remove("client_id");
    let (status, rotated) = token(&app, refresh, Some((&client, "secret"))).await;
    assert_eq!(status, 200, "{rotated}");
    assert_ne!(rotated["refresh_token"], issued["refresh_token"]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn legacy_exchanges_work_and_opt_in_covers_already_issued_refresh_tokens(pool: PgPool) {
    let (id, client, code) = fixture(&pool).await;
    assert_eq!(
        application::token_endpoint_auth_method(&pool, id)
            .await
            .unwrap(),
        Method::None
    );
    let app = vc_api::router(support::state(pool.clone(), support::fake()));
    let (status, issued) = token(
        &app,
        json!({"grant_type":"authorization_code",
        "client_id":client,"redirect_uri":REDIRECT,"code":code}),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, refreshed) = token(
        &app,
        json!({"grant_type":"refresh_token",
        "refresh_token":issued["refresh_token"]}),
        None,
    )
    .await;
    assert_eq!(status, 200);
    set_method(&pool, id, Method::ClientSecretBasic).await;
    let request = json!({"grant_type":"refresh_token","refresh_token":refreshed["refresh_token"]});
    assert_eq!(
        token(&app, request.clone(), None).await.1["error"],
        "invalid_client"
    );
    assert_eq!(token(&app, request, Some((&client, "secret"))).await.0, 200);
    // Unrelated edits preserve the setting, which is exposed in metadata.
    application::patch(
        &pool,
        id,
        &application::Changes {
            client_name: Some(Some("renamed".into())),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let details = oauth2_clients::details(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        oauth2_clients::render(&details)["token_endpoint_auth_method"],
        "client_secret_basic"
    );
}

#[test]
fn activation_boundary_is_inclusive_and_unset_preserves_legacy_default() {
    let now = OffsetDateTime::now_utc();
    assert_eq!(Method::registration_default(None, now), Method::None);
    assert_eq!(
        Method::registration_default(Some(now + Duration::seconds(1)), now),
        Method::None
    );
    assert_eq!(
        Method::registration_default(Some(now), now),
        Method::ClientSecretBasic
    );
    assert_eq!(
        Method::registration_default(Some(now - Duration::seconds(1)), now),
        Method::ClientSecretBasic
    );
}

#[test]
fn metadata_accepts_only_supported_methods_and_distinguishes_omission() {
    for invalid in [json!(null), json!(true), json!("unknown"), json!(1)] {
        let body = json!({"redirect_uris":[], "token_endpoint_auth_method":invalid});
        let registration: Registration = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(
            oauth2_clients::validated(registration).unwrap_err().error,
            "invalid_client_metadata"
        );
        assert_eq!(
            oauth2_clients::changes(body.as_object().unwrap())
                .unwrap_err()
                .error,
            "invalid_client_metadata"
        );
    }
    for method in ["none", "client_secret_basic"] {
        let body = json!({"redirect_uris":[], "token_endpoint_auth_method":method});
        let registration = serde_json::from_value(body.clone()).unwrap();
        assert!(
            oauth2_clients::validated(registration)
                .unwrap()
                .token_endpoint_auth_method
                .is_some()
        );
        assert!(
            oauth2_clients::changes(body.as_object().unwrap())
                .unwrap()
                .token_endpoint_auth_method
                .is_some()
        );
    }
    assert!(
        oauth2_clients::changes(json!({}).as_object().unwrap())
            .unwrap()
            .token_endpoint_auth_method
            .is_none()
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn registration_persists_scheduled_default_and_respects_explicit_choice(pool: PgPool) {
    let now = OffsetDateTime::now_utc();
    for (from, explicit, expected) in [
        (None, None, Method::None),
        (Some(now + Duration::days(1)), None, Method::None),
        (
            Some(now - Duration::days(1)),
            None,
            Method::ClientSecretBasic,
        ),
        (
            Some(now - Duration::days(1)),
            Some(Method::None),
            Method::None,
        ),
        (
            None,
            Some(Method::ClientSecretBasic),
            Method::ClientSecretBasic,
        ),
    ] {
        let state = support::state(pool.clone(), support::fake())
            .with_token_endpoint_auth_default_from(from);
        let mut new = oauth2_clients::validated(Registration {
            redirect_uris: Some(vec![]),
            ..Default::default()
        })
        .unwrap();
        new.token_endpoint_auth_method = explicit;
        let created = oauth2_clients::create(&state, 1, &new).await.unwrap();
        assert_eq!(created.token_endpoint_auth_method, expected);
        let application = application::find_by_client_id(&pool, &created.client_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            application::token_endpoint_auth_method(&pool, application.id)
                .await
                .unwrap(),
            expected
        );
    }
}
