mod support;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{fake, mint_app, state};
use tower::ServiceExt;

const URI: &str = "/oauth2/clients/@me";
const OWNER_DISCORD_ID: i64 = 500_000_000_000_000_001;

/// An application owned by `owner_discord_id`, and the id of the account created for
/// it. The account is what an application token's subject is.
async fn insert_application(pool: &PgPool, owner_discord_id: i64, name: &str) -> (i64, i32) {
    let id = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (client_id, client_name, owner_discord_id, inserted_at, updated_at,
              public_key, private_key)
           VALUES (gen_random_uuid(), $1, $2, now(), now(), '\x00'::bytea, '\x00'::bytea)
           RETURNING id"#,
        name,
        owner_discord_id
    )
    .fetch_one(pool)
    .await
    .expect("an application");

    let account = sqlx::query_scalar!(
        "INSERT INTO users (status, application_id, inserted_at, updated_at)
         VALUES (NULL, $1, now(), now()) RETURNING id",
        id
    )
    .fetch_one(pool)
    .await
    .expect("the application's account");

    (id, account)
}

async fn patch(pool: PgPool, token: String, body: Value) -> u16 {
    let req = axum::http::Request::builder()
        .method("PATCH")
        .uri(URI)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    vc_api::router(state(pool, fake()))
        .oneshot(req)
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn secret_rotation_invalidates_the_previous_secret(pool: PgPool) {
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "before").await;
    sqlx::query("UPDATE applications SET client_secret = 'old-test-secret' WHERE id = $1")
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    assert_eq!(
        patch(pool.clone(), token.clone(), json!({"client_secret": true})).await,
        204
    );
    let client_id: String =
        sqlx::query_scalar("SELECT client_id::text FROM applications WHERE id = $1")
            .bind(application)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        vc_core::application::verify_secret(&pool, &client_id, "old-test-secret")
            .await
            .unwrap()
            .is_none(),
        "PATCH returned 204 but the old client secret still authenticates"
    );
    let response = support::get(
        vc_api::router(state(pool.clone(), fake())),
        URI,
        Some(&token),
    )
    .await;
    assert_eq!(response.status, 200);
    let new_secret = response.body["client_secret"].as_str().unwrap();
    assert_ne!(new_secret, "old-test-secret");
    assert!(
        vc_core::application::verify_secret(&pool, &client_id, new_secret)
            .await
            .unwrap()
            .is_some()
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn disjoint_patches_preserve_both_changes(pool: PgPool) {
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "before").await;
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM applications WHERE id = $1 FOR UPDATE")
        .bind(application)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let one = tokio::spawn(patch(
        pool.clone(),
        token.clone(),
        json!({"client_name": "after"}),
    ));
    let two = tokio::spawn(patch(
        pool.clone(),
        token,
        json!({"client_uri": "https://after.example"}),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND (query LIKE 'UPDATE applications%' OR query LIKE '%FROM applications WHERE id = $1%')")
                .fetch_one(&pool).await.unwrap();
            if waiting == 2 { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("both patches reached the locked application row");
    blocker.commit().await.unwrap();
    assert_eq!(one.await.unwrap(), 204);
    assert_eq!(two.await.unwrap(), 204);
    let values: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT client_name, client_uri FROM applications WHERE id = $1")
            .bind(application)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        values,
        (Some("after".into()), Some("https://after.example".into())),
        "One successful PATCH reverted the other"
    );
}

async fn stored_secret(pool: &PgPool, application: i64) -> Option<String> {
    sqlx::query_scalar("SELECT client_secret FROM applications WHERE id = $1")
        .bind(application)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn false_or_absent_rotation_preserves_secret(pool: PgPool) {
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "before").await;
    sqlx::query("UPDATE applications SET client_secret = 'old-test-secret' WHERE id = $1")
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    for body in [
        json!({"client_secret": false, "client_name": "after"}),
        json!({"client_uri": "https://after.example"}),
    ] {
        assert_eq!(patch(pool.clone(), token.clone(), body).await, 204);
        assert_eq!(
            stored_secret(&pool, application).await.as_deref(),
            Some("old-test-secret")
        );
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn invalid_rotation_types_and_metadata_do_not_change_secret(pool: PgPool) {
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "before").await;
    sqlx::query("UPDATE applications SET client_secret = 'old-test-secret' WHERE id = $1")
        .bind(application)
        .execute(&pool)
        .await
        .unwrap();
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    for value in [Value::Null, json!("true"), json!(1), json!([]), json!({})] {
        let req = axum::http::Request::builder()
            .method("PATCH")
            .uri(URI)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::from(
                json!({"client_secret": value, "client_name": "after"}).to_string(),
            ))
            .unwrap();
        let response = vc_api::router(state(pool.clone(), fake()))
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 400);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "invalid_client_metadata");
    }
    assert_eq!(
        patch(
            pool.clone(),
            token,
            json!({"client_secret": true, "redirect_uris": ["ftp://invalid.example"]})
        )
        .await,
        400
    );
    assert_eq!(
        stored_secret(&pool, application).await.as_deref(),
        Some("old-test-secret")
    );
    let name: String = sqlx::query_scalar("SELECT client_name FROM applications WHERE id = $1")
        .bind(application)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "before");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn oversized_redirect_updates_preserve_the_existing_list(pool: PgPool) {
    let (application, account) = insert_application(&pool, OWNER_DISCORD_ID, "app").await;
    let token = mint_app(&pool, account, &["oauth2.register"]).await;
    let original = vec!["https://example.com/callback".to_string()];
    assert_eq!(
        patch(
            pool.clone(),
            token.clone(),
            json!({"redirect_uris":original})
        )
        .await,
        204
    );
    for uris in [
        vec![format!("https://example.com/{}", "a".repeat(256))],
        (0..21)
            .map(|i| format!("https://example.com/{i}/{}", "a".repeat(180)))
            .collect(),
    ] {
        assert_eq!(
            patch(pool.clone(), token.clone(), json!({"redirect_uris":uris})).await,
            400
        );
        let stored: Vec<String> =
            sqlx::query_scalar("SELECT redirect_uri FROM redirect_uris WHERE application_id = $1")
                .bind(application)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(stored, original);
    }
}

#[test]
fn registration_and_edits_share_redirect_length_limits() {
    use vc_api::routes::oauth2_clients::{Registration, changes, validated};
    fn accepted(uris: Vec<String>, expected: bool) {
        assert_eq!(
            validated(Registration {
                redirect_uris: Some(uris.clone()),
                ..Default::default()
            })
            .is_ok(),
            expected
        );
        assert_eq!(
            changes(json!({"redirect_uris":uris}).as_object().unwrap()).is_ok(),
            expected
        );
    }
    let uri = |n: usize| format!("https://example.com/{}", "a".repeat(n - 20));
    // The prefix is 20 characters; test exact boundaries, including separators.
    assert_eq!(uri(255).chars().count(), 255);
    accepted(vec![uri(255)], true);
    accepted(vec![uri(256)], false);
    let mut list = vec![uri(249); 8];
    list[7] = uri(250);
    assert_eq!(list.join("\n").chars().count(), 2000);
    accepted(list.clone(), true);
    list[7].push('a');
    accepted(list, false);
}
