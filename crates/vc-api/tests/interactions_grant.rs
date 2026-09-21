//! The `/grant` command: what a guild decides about applications that want to
//! issue from its pool.
//!
//! Additions rather than ports — the Elixir had no command that allowed an
//! application anything, because a grant was only ever a side effect of redeeming
//! an authorization code. What they pin is the shape the commands beside it have:
//! ephemeral answers, and the administrator bit asked for the way `/issue` asks
//! for it.
//!
//! The division the screens draw is pinned too: the approval is a typed code,
//! which only the application that asked can put in front of a person, and the
//! taking-back is a button on the list of what the guild has allowed. That list
//! holds what the guild can act on, so a pending ask — the application's own
//! business — is not on it.

mod support;

use std::sync::Arc;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    DEFAULT_GUILD, DEFAULT_PERMISSIONS, Recorded, Response, button_from_guild, client_id_of, fake,
    insert_application, interaction, state, state_with_notifier,
};
use vc_api::custom_id::ui::grant::{Page, page_custom_id, revoke_custom_id};

/// The administrator the interactions come from. The id is a Discord one, and the
/// command never resolves it to an account.
const ADMIN: i64 = 900_000_000_000_000_001;
const NOT_ADMIN: &str = "0";
const SCOPES: &[&str] = &["vc.issue"];

/// The owner of the applications this file authorizes, and its neighbours: every
/// application has an account of its own, so no two may share one.
const FIRST_OWNER: i64 = 910_000_000_000_000_001;

/// What the list says when the guild has allowed nobody. The approval's own
/// sentence is in here, because a guild that has allowed nobody is the guild most
/// likely to be looking for it.
const EMPTY: &str = "発行を許可しているアプリケーションはありません。申請が来たときは、\
                     アプリケーションが表示するコードを `/grant approve code:` に入れて承認します。";

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// The same router, with the decisions told to `notified` rather than to
/// nobody.
fn router_with(pool: PgPool, notified: Arc<Recorded>) -> Router {
    vc_api::router(state_with_notifier(pool, fake(), notified))
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

/// An application the guild has allowed to issue, which is the row the list shows
/// and the button names.
async fn authorized(pool: &PgPool, owner: i64, name: &str) -> (i64, String) {
    let application = insert_application(pool, owner, name).await;

    vc_core::grant::allow_in_guild(
        pool,
        application,
        DEFAULT_GUILD,
        SCOPES,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a grant");

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

fn approve_options(code: &str) -> Value {
    json!([{
        "name": "approve",
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

/// The `custom_id` of every button the screen offers, in order.
fn buttons(response: &Response) -> Vec<String> {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .filter(|child| child["type"] == 1)
        .flat_map(|row| {
            row["components"]
                .as_array()
                .expect("an action row's children")
                .iter()
                .filter_map(|component| component["custom_id"].as_str().map(str::to_owned))
                .collect::<Vec<String>>()
        })
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

/// An ask the guild has not answered is not a permission, so it is not a row:
/// the application holds the `device_code` it polls with and the `user_code` it
/// shows the administrator, and the list is what the guild may act on.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_ask_is_not_on_the_list(pool: PgPool) {
    fixture(&pool).await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), [EMPTY]);
    assert!(buttons(&response).is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_shows_who_may_issue(pool: PgPool) {
    let (_, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "**発行を許可しているアプリケーション** (1件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。"
                .to_string(),
            format!("**an application**\n`{client_id}`"),
        ]
    );
    assert_eq!(buttons(&response), [revoke_custom_id(&client_id)]);
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

/// And the application it was written for is what the list then shows.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_puts_the_application_on_the_list(pool: PgPool) {
    let (application, user_code) = fixture(&pool).await;
    let client_id = client_id_of(&pool, application).await;

    interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    let response = interaction(
        router(pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(
        texts(&response)[1],
        format!("**an application**\n`{client_id}`")
    );
    assert_eq!(buttons(&response), [revoke_custom_id(&client_id)]);
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

/// The button takes the issuing scope back, tells the application the way an
/// approval does, and redraws the screen it came from — which no longer has the
/// row.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pressing_revoke_takes_the_permission_back(pool: PgPool) {
    let (application, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;
    let notified = Arc::new(Recorded::default());

    let response = interaction(
        router_with(pool.clone(), notified.clone()),
        button_from_guild(json!({ "custom_id": revoke_custom_id(&client_id) }), ADMIN),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(response.body["type"], 7, "a redraw: {}", response.body);
    assert_eq!(texts(&response), [EMPTY]);
    assert!(!allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(
        notified.grant_decisions(),
        [(application, DEFAULT_GUILD)],
        "the taking-back"
    );
}

/// The bit is asked for again on the press: a message stays in a channel, and what
/// a press carries is the presser's own permissions rather than the ones the
/// screen was drawn with.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_press_from_a_member_without_the_bit_changes_nothing(pool: PgPool) {
    let (application, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;

    let payload = json!({
        "type": 3,
        "data": { "custom_id": revoke_custom_id(&client_id), "component_type": 2 },
        "member": { "user": { "id": ADMIN.to_string() }, "permissions": NOT_ADMIN },
        "guild_id": DEFAULT_GUILD.to_string(),
    });

    let response = interaction(router(pool.clone()), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: 実行には管理者権限が必要です。"]);
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
}

/// Five rows to a page and four arrows around them, the claim list's and contract
/// list's arithmetic: the sixth application is on page two, and the page a button
/// names is the page the redraw shows.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_pages(pool: PgPool) {
    let mut client_ids = Vec::new();

    for index in 0..6 {
        let (_, client_id) = authorized(
            &pool,
            FIRST_OWNER + index,
            &format!("an application {index}"),
        )
        .await;

        client_ids.push(client_id);
    }

    let first = interaction(
        router(pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(
        texts(&first)[0],
        "**発行を許可しているアプリケーション** (6件)\n\
         取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。"
    );

    // The five rows' buttons, then the arrows: there is no page one from page one,
    // which is what a disabled arrow says.
    assert_eq!(
        buttons(&first)[5..],
        [
            "disabled-0".to_string(),
            "disabled-1".to_string(),
            page_custom_id(Page::Next, 2),
            page_custom_id(Page::Last, 2),
        ]
    );

    let second = interaction(
        router(pool.clone()),
        button_from_guild(json!({ "custom_id": page_custom_id(Page::Next, 2) }), ADMIN),
    )
    .await;

    assert_eq!(second.body["type"], 7, "a redraw: {}", second.body);
    assert_eq!(
        texts(&second),
        [
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。"
                .to_string(),
            format!("**an application 0**\n`{}`", client_ids[0]),
        ]
    );
    assert_eq!(
        buttons(&second)[1..],
        [
            page_custom_id(Page::First, 1),
            page_custom_id(Page::Previous, 1),
            "disabled-2".to_string(),
            "disabled-3".to_string(),
        ]
    );

    // Every arrow, pressed: the four are one call with the page in the id, so what a press
    // has to show is the page the button named. ⏪ and ⏮️ are the two ways back from here —
    // the arrows after this page's own row, which is where a button of a row is not.
    let page_two = buttons(&second);
    let arrows = &page_two[1..];

    for arrow in [&arrows[0], &arrows[1]] {
        let back = interaction(
            router(pool.clone()),
            button_from_guild(json!({ "custom_id": arrow }), ADMIN),
        )
        .await;

        assert_eq!(back.status, 200, "body: {}", back.body);
        assert_eq!(back.body["type"], 7, "a redraw: {}", back.body);
        assert_eq!(
            texts(&back)[0],
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。"
        );
        assert_eq!(
            buttons(&back).len(),
            5 + 4,
            "the page's five and the arrows"
        );
        assert_eq!(
            buttons(&back)[7],
            page_custom_id(Page::Next, 2),
            "⏭️ has somewhere to go again"
        );
    }

    // And ⏩ from the first screen: the end of the list, which is where ⏭️ went.
    let last = interaction(
        router(pool),
        button_from_guild(json!({ "custom_id": buttons(&first)[8] }), ADMIN),
    )
    .await;

    assert_eq!(last.status, 200, "body: {}", last.body);
    assert_eq!(
        texts(&last),
        [
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すと、そのアプリケーションはこのサーバーの発行枠から発行できなくなります。"
                .to_string(),
            format!("**an application 0**\n`{}`", client_ids[0]),
        ]
    );
}

/// A decision is told to the application that asked, and the yes and the
/// taking-back travel the same way: a device that named a webhook learns what
/// its token may still do without polling for the difference.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_decision_pings_the_application(pool: PgPool) {
    let (application, user_code) = fixture(&pool).await;
    let notified = Arc::new(Recorded::default());

    interaction(
        router_with(pool.clone(), notified.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    assert_eq!(
        notified.grant_decisions(),
        [(application, DEFAULT_GUILD)],
        "the approval"
    );

    let client_id = client_id_of(&pool, application).await;

    interaction(
        router_with(pool.clone(), notified.clone()),
        button_from_guild(json!({ "custom_id": revoke_custom_id(&client_id) }), ADMIN),
    )
    .await;

    assert_eq!(
        notified.grant_decisions(),
        [(application, DEFAULT_GUILD), (application, DEFAULT_GUILD)],
        "and the taking-back"
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
