//! The `/grant` command: what a guild decides about applications that want to
//! issue from its pool.
//!
//! Additions rather than ports — the Elixir had no command that allowed an
//! application anything, because a grant was only ever a side effect of redeeming
//! an authorization code. What they pin is the shape the commands beside it have:
//! ephemeral answers, and the administrator bit asked for the way `/issue` asks
//! for it.
//!
//! There are no buttons here on purpose: the approval names the application's
//! own `user_code`, typed rather than pressed, so an approval always answers an
//! ask — a permission nobody asked for cannot be written.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_GUILD, DEFAULT_PERMISSIONS, Response, fake, insert_application, interaction, state,
};

/// The administrator the interactions come from. The id is a Discord one, and the
/// command never resolves it to an account.
const ADMIN: i64 = 900_000_000_000_000_001;
const NOT_ADMIN: &str = "0";
const SCOPES: &[&str] = &["vc.issue"];

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// An application with a name, which is what the screens show, and its pending
/// ask with the code the guild types.
async fn fixture(pool: &PgPool) -> (i64, String) {
    let application = insert_application(pool, 900_000_000_000_000_002, "an application").await;
    let asked = vc_core::grant::request_grant(
        pool,
        application,
        DEFAULT_GUILD,
        &SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect::<Vec<_>>(),
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("an ask");

    (application, asked.user_code)
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

fn approve_options(code: &str) -> Value {
    json!([{
        "name": "approve",
        "type": 1,
        "options": [{ "name": "code", "type": 3, "value": code }],
    }])
}

fn revoke_options(code: &str) -> Value {
    json!([{
        "name": "revoke",
        "type": 1,
        "options": [{ "name": "code", "type": 3, "value": code }],
    }])
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

async fn request_status(pool: &PgPool, application: i64) -> String {
    vc_core::grant::requests_of(pool, application)
        .await
        .expect("the asks")[0]
        .status
        .clone()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_shows_the_code_to_type(pool: PgPool) {
    let (_, user_code) = fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "**発行の申請** (1件)\n`/grant approve code:` にコードを入れて承認します。".to_string(),
            format!("**an application**\n`{user_code}`\n要求スコープ: vc.issue"),
        ]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_the_code_writes_the_grant(pool: PgPool) {
    let (application, user_code) = fixture(&pool).await;

    let response = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["発行を許可しました。"]);
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(request_status(&pool, application).await, "approved");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_that_names_nothing_pending_is_refused(pool: PgPool) {
    fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options("deadbeef")),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: そのコードの申請はこのサーバーにありません。"]
    );
}

/// An approval answers the ask's own scopes, never anything else: the grant the
/// device polls for is written from the request it made.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_grant_carries_the_asked_scopes(pool: PgPool) {
    let (application, user_code) = fixture(&pool).await;

    interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    let scopes = sqlx::query_scalar!(
        r#"SELECT s.scope::text AS "scope!" FROM grant_scopes s
             JOIN grants g ON g.id = s.grant_id
            WHERE g.application_id = $1 AND g.guild_id = $2"#,
        application,
        DEFAULT_GUILD
    )
    .fetch_all(&pool)
    .await
    .expect("the scopes");

    assert_eq!(scopes, ["vc.issue"]);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_list_with_nothing_pending_says_so(pool: PgPool) {
    insert_application(&pool, 900_000_000_000_000_002, "an application").await;

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

/// Revoking by the pending code un-asks it: the application's poll reads the ask
/// as gone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_a_pending_code_unasks_it(pool: PgPool) {
    let (application, user_code) = fixture(&pool).await;

    let response = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, revoke_options(&user_code)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["発行の許可を取り消しました。"]);
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);

    let listed = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(
        texts(&listed),
        ["発行を申請しているアプリケーションはありません。"]
    );
}

/// Revoking by the granted `client_id` drops the issuing scope, and a guild
/// token already issued stops issuing because its scopes are read from the
/// grant.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn revoking_a_client_id_ungrants_it(pool: PgPool) {
    use support::client_id_of;

    let (application, user_code) = fixture(&pool).await;
    let client_id = client_id_of(&pool, application).await;

    interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    assert!(allowed(&pool, application, DEFAULT_GUILD).await);

    let response = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, revoke_options(&client_id)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["発行の許可を取り消しました。"]);
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_revoke_that_names_nothing_is_refused(pool: PgPool) {
    fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, revoke_options("deadbeef")),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: そのコードの申請も許可もこのサーバーにありません。"]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_command_needs_the_administrator_bit(pool: PgPool) {
    let (_, user_code) = fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, NOT_ADMIN, approve_options(&user_code)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: 実行には管理者権限が必要です。"]);
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
