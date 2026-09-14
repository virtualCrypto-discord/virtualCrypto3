//! The `/grant` command: what a guild decides about applications that want to
//! issue from its pool.
//!
//! Additions rather than ports — the Elixir had no command that allowed an
//! application anything, because a grant was only ever a side effect of redeeming
//! an authorization code. What they pin is the shape the commands beside it have:
//! an ephemeral answer, a `custom_id` that carries the subject, and the
//! administrator bit asked for twice — once when the command runs and once when
//! its button is pressed.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_GUILD, DEFAULT_PERMISSIONS, Response, client_id_of, fake, insert_application,
    interaction, state,
};
use vc_api::custom_id::ui::grant::{Action, custom_id};

/// The administrator the interactions come from. The id is a Discord one, and the
/// command never resolves it to an account.
const ADMIN: i64 = 900_000_000_000_000_001;
const NOT_ADMIN: &str = "0";

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// An application with a name, which is what the screens show.
async fn fixture(pool: &PgPool) -> (i64, String) {
    let application = insert_application(pool, 900_000_000_000_000_002, "an application").await;

    (application, client_id_of(pool, application).await)
}

fn grant_from_guild(user: i64, permissions: &str, options: Value) -> Value {
    json!({
        "type": 2,
        "data": { "name": "grant", "options": options },
        "member": { "user": { "id": user.to_string() }, "permissions": permissions },
        "guild_id": DEFAULT_GUILD.to_string(),
    })
}

fn list_options() -> Value {
    json!([{ "name": "list", "type": 1 }])
}

fn allow_options(client_id: &str) -> Value {
    json!([{
        "name": "allow",
        "type": 1,
        "options": [{ "name": "client_id", "type": 3, "value": client_id }],
    }])
}

fn press(user: i64, permissions: &str, action: Action, subject: i64) -> Value {
    json!({
        "type": 3,
        "data": { "custom_id": custom_id(action, subject), "component_type": 2 },
        "member": { "user": { "id": user.to_string() }, "permissions": permissions },
        "guild_id": DEFAULT_GUILD.to_string(),
    })
}

/// What the one container says, which is everything a screen has to say.
fn texts(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter_map(|child| child["content"].as_str().map(str::to_owned))
        .collect()
}

/// Whether the guild allows this application to issue: the grant and the scope
/// together, which is what the issuing endpoint reads.
async fn allowed(pool: &PgPool, application: i64, guild_id: i64) -> bool {
    sqlx::query_scalar!(
        r#"SELECT EXISTS(
             SELECT 1 FROM grant_scopes s
               JOIN grants g ON g.id = s.grant_id
              WHERE g.application_id = $1 AND g.guild_id = $2
                AND s.scope = 'vc.issue'::virtual_crypto_scope_type) AS "exists!""#,
        application,
        guild_id
    )
    .fetch_one(pool)
    .await
    .expect("the check")
}

async fn request_status(pool: &PgPool, request_id: i64) -> String {
    sqlx::query_scalar!(
        "SELECT status FROM grant_requests WHERE id = $1",
        request_id
    )
    .fetch_one(pool)
    .await
    .expect("the request")
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_confirmation_names_the_application(pool: PgPool) {
    let (_, client_id) = fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, allow_options(&client_id)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(4), "a new ephemeral message");
    assert_eq!(
        texts(&response),
        [
            format!("**an application**\n`{client_id}`"),
            "このサーバーのプールからの発行を許可しますか？".to_string(),
        ]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn allowing_writes_the_grant_and_its_scope(pool: PgPool) {
    let (application, client_id) = fixture(&pool).await;

    interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, allow_options(&client_id)),
    )
    .await;

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Allow, application),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7), "the screen is replaced");
    assert_eq!(texts(&response), ["発行を許可しました。"]);
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_confirmation_can_be_dismissed(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Cancel, application),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["許可しませんでした。"]);
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_application_that_is_not_there_is_refused(pool: PgPool) {
    fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(
            ADMIN,
            DEFAULT_PERMISSIONS,
            allow_options("2f1c2f4e-9a11-4d2b-8c3e-5f6a7b8c9d0e"),
        ),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: アプリケーションが見つかりません。"]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_list_with_nothing_pending_says_so(pool: PgPool) {
    fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["発行を申請しているアプリケーションはありません。"]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_request_writes_the_grant(pool: PgPool) {
    let (application, client_id) = fixture(&pool).await;

    let request = vc_core::grant::request_grant(
        &pool,
        application,
        DEFAULT_GUILD,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a request");

    let listed = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(listed.status, 200, "body: {}", listed.body);
    assert_eq!(
        texts(&listed),
        [
            "**発行の申請** (1件)".to_string(),
            format!("**an application**\n`{client_id}`"),
        ]
    );

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Approve, request),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], json!(7), "the list is redrawn");
    assert_eq!(
        texts(&response),
        [
            "発行を許可しました。".to_string(),
            "発行を申請しているアプリケーションはありません。".to_string(),
        ]
    );
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(request_status(&pool, request).await, "approved");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn denying_a_request_leaves_it_ungranted(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let request = vc_core::grant::request_grant(
        &pool,
        application,
        DEFAULT_GUILD,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a request");

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Deny, request),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "申請を拒否しました。".to_string(),
            "発行を申請しているアプリケーションはありません。".to_string(),
        ]
    );
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(request_status(&pool, request).await, "denied");
}

/// A decided request is not decided again: the second press says so rather than
/// writing a grant the guild did not answer for.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_request_is_answered_once(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let request = vc_core::grant::request_grant(
        &pool,
        application,
        DEFAULT_GUILD,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a request");

    interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Deny, request),
    )
    .await;

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, DEFAULT_PERMISSIONS, Action::Approve, request),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert!(
        texts(&response)
            .iter()
            .any(|text| text == "エラー: この申請はすでに処理されています。"),
        "body: {}",
        response.body
    );
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(request_status(&pool, request).await, "denied");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_command_needs_the_administrator_bit(pool: PgPool) {
    let (_, client_id) = fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, NOT_ADMIN, allow_options(&client_id)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: 実行には管理者権限が必要です。"]);
}

/// The permission is asked for again on the press, because the button outlives
/// the message it came in: a person demoted in between still holds it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_button_needs_the_administrator_bit(pool: PgPool) {
    let (application, _) = fixture(&pool).await;

    let response = interaction(
        router(pool.clone()),
        press(ADMIN, NOT_ADMIN, Action::Allow, application),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: 実行には管理者権限が必要です。"]);
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_command_is_refused_in_a_direct_message(pool: PgPool) {
    fixture(&pool).await;

    let payload = json!({
        "type": 2,
        "data": { "name": "grant", "options": list_options() },
        "user": { "id": ADMIN.to_string() },
    });

    let response = interaction(router(pool), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: DMでは実行できません。"]);
}
