//! Reading back what an application is answered with.
//!
//! Additions rather than ports: OAuth2 is the part of the migration with no
//! Elixir test to follow.

use sqlx::PgPool;
use uuid::Uuid;
use vc_api::routes::oauth2_clients::{details, render};

/// An application, the account that owns it, and one redirect URI.
async fn application(pool: &PgPool) -> (i64, i32) {
    let client_id = Uuid::new_v4();

    let application_id = sqlx::query_scalar!(
        r#"INSERT INTO applications
             (status, client_id, client_secret, response_types, grant_types,
              application_type, client_name, client_uri, webhook_url,
              public_key, private_key, inserted_at, updated_at)
           VALUES (0, $1, 'a-secret',
                   ARRAY['code']::openid_connect_response_types[],
                   ARRAY['authorization_code']::openid_connect_grant_types[],
                   'web', 'An Application', 'https://app.example',
                   'https://app.example/hook',
                   '\x01abff'::bytea, '\x00'::bytea,
                   now()::timestamp(0), now()::timestamp(0))
        RETURNING id"#,
        client_id
    )
    .fetch_one(pool)
    .await
    .expect("insert the application");

    sqlx::query!(
        "INSERT INTO redirect_uris (application_id, redirect_uri, inserted_at, updated_at)
         VALUES ($1, 'https://app.example/callback', now()::timestamp(0), now()::timestamp(0))",
        application_id
    )
    .execute(pool)
    .await
    .expect("insert the redirect uri");

    // The account that owns it: `users.application_id` is the only link.
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (discord_id, status, application_id, inserted_at, updated_at)
         VALUES (100000000000000001, 0, $1, now()::timestamp(0), now()::timestamp(0))
        RETURNING id",
        application_id
    )
    .fetch_one(pool)
    .await
    .expect("insert the account");

    (application_id, user_id)
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_is_read_back_whole(pool: PgPool) {
    let (application_id, user_id) = application(&pool).await;

    let found = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(found.user_id, user_id);
    assert_eq!(found.client_name.as_deref(), Some("An Application"));
    assert_eq!(found.redirect_uris, ["https://app.example/callback"]);
    assert_eq!(found.grant_types, ["authorization_code"]);
    assert_eq!(found.response_types, ["code"]);
    assert_eq!(found.application_type, "web");
    assert_eq!(found.client_secret.as_deref(), Some("a-secret"));
    assert_eq!(
        found.subscribed_events,
        vec![2, 3, 4],
        "the column defaults to everything, spelled out"
    );
}

/// The two halves together, which is where the encodings live: strings for the
/// ids, lowercase hex for the key, a null for what is absent.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn and_answered_with_the_promised_encodings(pool: PgPool) {
    let (application_id, user_id) = application(&pool).await;

    let found = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    let answered = render(&found);

    assert_eq!(answered["user_id"], user_id.to_string());
    assert_eq!(answered["discord_user_id"], "100000000000000001");
    assert_eq!(answered["public_key"], "01abff", "lowercase hex");
    assert_eq!(answered["client_secret_expires_at"], 0);
    assert_eq!(answered["redirect_uris"][0], "https://app.example/callback");
    assert_eq!(
        answered["discord_support_server_invite_slug"],
        serde_json::Value::Null,
        "absent, and present as a null"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_that_is_not_there_is_not_an_error(pool: PgPool) {
    let found = details(&pool, 999_999).await.expect("an answer");

    assert_eq!(found, None);
}

/// An edit writes what it was given and leaves the rest. The two-level option is
/// what makes "not in the request" and "explicitly null" different operations.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_edit_writes_what_it_was_given(pool: PgPool) {
    let (application_id, _) = application(&pool).await;

    vc_core::application::patch(
        &pool,
        application_id,
        &vc_core::application::Changes {
            client_name: Some(Some("Renamed".to_owned())),
            logo_uri: Some(None),
            redirect_uris: Some(vec!["https://app.example/one".to_owned()]),
            ..Default::default()
        },
    )
    .await
    .expect("an edit");

    let after = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(after.client_name.as_deref(), Some("Renamed"));
    assert_eq!(
        after.logo_uri, None,
        "cleared, because the request said null"
    );
    assert_eq!(
        after.client_uri.as_deref(),
        Some("https://app.example"),
        "untouched, because the request did not mention it"
    );
    assert_eq!(after.redirect_uris, ["https://app.example/one"]);
}

/// The redirect URIs are replaced rather than added to, so an edit that sends one
/// URI leaves one.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn redirect_uris_are_replaced_wholesale(pool: PgPool) {
    let (application_id, _) = application(&pool).await;

    vc_core::application::patch(
        &pool,
        application_id,
        &vc_core::application::Changes {
            redirect_uris: Some(vec![
                "https://app.example/a".to_owned(),
                "https://app.example/b".to_owned(),
            ]),
            ..Default::default()
        },
    )
    .await
    .expect("an edit");

    let after = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(
        after.redirect_uris.len(),
        2,
        "two, and not the one that was there before them"
    );
}

/// The subscription is replaced rather than added to, like the redirect URIs:
/// an edit that sends one event leaves one, and what is not mentioned is left
/// alone rather than cleared.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn subscribed_events_are_replaced_wholesale(pool: PgPool) {
    let (application_id, _) = application(&pool).await;

    vc_core::application::patch(
        &pool,
        application_id,
        &vc_core::application::Changes {
            subscribed_events: Some(vec![2]),
            ..Default::default()
        },
    )
    .await
    .expect("an edit");

    let after = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(after.subscribed_events, [2]);

    vc_core::application::patch(
        &pool,
        application_id,
        &vc_core::application::Changes {
            client_name: Some(Some("Renamed".to_owned())),
            ..Default::default()
        },
    )
    .await
    .expect("another edit");

    let kept = details(&pool, application_id)
        .await
        .expect("an answer")
        .expect("the application");

    assert_eq!(kept.subscribed_events, [2], "unmentioned, so untouched");
}

/// `Repo.update_all/2` applies no changeset and therefore no timestamp, so an edit
/// leaves `updated_at` where it was. Observable, and reproduced.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_edit_does_not_touch_updated_at(pool: PgPool) {
    let (application_id, _) = application(&pool).await;

    let before = sqlx::query!(
        "SELECT updated_at FROM applications WHERE id = $1",
        application_id
    )
    .fetch_one(&pool)
    .await
    .expect("the row")
    .updated_at;

    vc_core::application::patch(
        &pool,
        application_id,
        &vc_core::application::Changes {
            client_name: Some(Some("Renamed".to_owned())),
            ..Default::default()
        },
    )
    .await
    .expect("an edit");

    let after = sqlx::query!(
        "SELECT updated_at FROM applications WHERE id = $1",
        application_id
    )
    .fetch_one(&pool)
    .await
    .expect("the row")
    .updated_at;

    assert_eq!(after, before);
}
