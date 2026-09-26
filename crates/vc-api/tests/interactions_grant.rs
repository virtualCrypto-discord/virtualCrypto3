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
    insert_application, insert_currency, rendered_interaction as interaction, state,
    state_with_notifier,
};
use tower::ServiceExt;
use vc_api::custom_id::ui::grant::{Page, page_custom_id};

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

fn router(discord: std::sync::Arc<support::FakeDiscord>, pool: PgPool) -> Router {
    vc_api::router(state(pool, discord))
}

/// The same router, with the decisions told to `notified` rather than to
/// nobody.
fn router_with(
    discord: Arc<support::FakeDiscord>,
    pool: PgPool,
    notified: Arc<Recorded>,
) -> Router {
    vc_api::router(state_with_notifier(pool, discord, notified))
}

/// An application with a name, which is what the screens show, and its pending
/// ask with the code the guild types.
async fn fixture(pool: &PgPool) -> (i64, String) {
    let application = insert_application(pool, 900_000_000_000_000_002, "an application").await;
    let asked = vc_core::grant::request_grant(
        pool,
        application,
        vc_core::grant::Target::Guild(DEFAULT_GUILD),
        &SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect::<Vec<_>>(),
        &[],
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
        &[],
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
    json!([{ "name": "server", "type": 1 }])
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
    let discord = fake();
    fixture(&pool).await;

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), [EMPTY]);
    assert!(buttons(&response).is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn grant_screens_render_only_bound_bot_ids_as_mentions(pool: PgPool) {
    let discord = fake();
    use vc_api::custom_id::ui::grant as ids;
    use vc_core::grant::Target;

    const BOT: i64 = 700_000_000_000_000_001;
    const NAME: &str = "`\n<@987654321987654321>\r\n**trusted** @everyone <@&123>";
    let application = insert_application(&pool, FIRST_OWNER, "normal").await;
    let account = support::account_of(&pool, application).await;
    let client_id = client_id_of(&pool, application).await;
    let token = support::mint_app(&pool, account, &["oauth2.register"]).await;
    let http = router(discord.clone(), pool.clone());
    // This name is accepted through the real API. Only its presentation changes.
    let patch = axum::http::Request::builder()
        .method("PATCH")
        .uri("/oauth2/clients/@me")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(
            json!({"client_name": NAME}).to_string(),
        ))
        .unwrap();
    assert_eq!(http.clone().oneshot(patch).await.unwrap().status(), 204);

    for bound in [false, true] {
        let expected = if bound {
            vc_core::user::bind_bot(&pool, account, BOT).await.unwrap();
            format!("<@{BOT}>")
        } else {
            format!(
                "Bot未連携: `｀ <@987654321987654321>  **trusted** @everyone <@&123>`\nclient_id: `{client_id}`"
            )
        };
        let check = |response: &Response| {
            assert_eq!(response.status, 200, "{}", response.body);
            let text = texts(response).join("\n");
            assert!(text.contains(&expected), "{text}");
            // Mentions in code are literal text; outside code only the verified
            // binding may be rendered. Name-supplied backticks cannot escape it.
            let outside_code = text.split('`').step_by(2).collect::<String>();
            let outside_code = if bound {
                assert!(!text.contains(NAME));
                assert!(!text.contains(&client_id));
                outside_code.replace(&expected, "")
            } else {
                outside_code
            };
            assert!(!outside_code.contains("<@"), "{text}");
            assert!(!outside_code.contains("@everyone"), "{text}");
            assert!(!outside_code.contains("**trusted**"), "{text}");
            assert_eq!(
                response.body["data"]["allowed_mentions"]["parse"],
                json!([])
            );
        };
        for target in [Target::User(ADMIN), Target::Guild(DEFAULT_GUILD)] {
            let scope = match target {
                Target::User(_) => "vc.delegate.payments.create",
                Target::Guild(_) => "vc.issue",
            };
            let asked = vc_core::grant::request_grant(
                &pool,
                application,
                target,
                &[scope.into()],
                &[],
                600,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .unwrap();
            let command = |options| match target {
                Target::User(_) => from_dm(ADMIN, options),
                Target::Guild(_) => grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, options),
            };
            let review = interaction(
                discord.clone(),
                http.clone(),
                command(approve_options(&asked.user_code)),
            )
            .await;
            check(&review);
            let page = interaction(
                discord.clone(),
                http.clone(),
                press_as(command(json!([])), &ids::review_page_custom_id(asked.id, 1)),
            )
            .await;
            check(&page);
            interaction(
                discord.clone(),
                http.clone(),
                press_as(command(json!([])), &buttons(&review)[0]),
            )
            .await;
            let grant_id: i64 =
                sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE id=$1")
                    .bind(asked.id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let list = interaction(discord.clone(),
                http.clone(),
                command(json!([{"name": if matches!(target, Target::User(_)) { "user" } else { "server" }, "type":1}])),
            ).await;
            check(&list);
            let details = interaction(
                discord.clone(),
                http.clone(),
                press_as(command(json!([])), &ids::details_custom_id(grant_id, 1)),
            )
            .await;
            check(&details);
        }
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_shows_who_may_issue(pool: PgPool) {
    let discord = fake();
    let (app, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;
    let grant_id = vc_core::grant::grant_for(&pool, app, DEFAULT_GUILD)
        .await
        .unwrap()
        .unwrap();

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "**発行を許可しているアプリケーション** (1件)\n\
             取り消すのは選んだ許可だけです。同じアプリケーションへの他の許可は残ります。"
                .to_string(),
            format!(
                "Bot未連携: `an application`\nclient_id: `{client_id}`\nこの申請は、すべての通貨を操作できます。"
            ),
        ]
    );
    assert_eq!(
        buttons(&response),
        [vc_api::custom_id::ui::grant::revoke_one_custom_id(grant_id)]
    );
}

/// A grant narrowed to one currency reads as such: the screen names it by unit,
/// so the narrowing is visible rather than something only the API enforces. A
/// grant that named none beside it reads as 「すべての通貨」, which is the
/// difference the screen exists to show.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_names_the_currency_a_grant_is_narrowed_to(pool: PgPool) {
    let discord = fake();
    const CURRENCY: i64 = 7;

    insert_currency(&pool, CURRENCY, "nyan", "nyan", DEFAULT_GUILD, 500).await;

    let application = insert_application(&pool, FIRST_OWNER, "an application").await;
    vc_core::grant::allow_in_guild(
        &pool,
        application,
        DEFAULT_GUILD,
        SCOPES,
        &[CURRENCY],
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("a grant");
    let client_id = client_id_of(&pool, application).await;

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response)[1],
        format!(
            "Bot未連携: `an application`\nclient_id: `{client_id}`\nこの申請は、通貨 nyan だけを操作できます。"
        )
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_the_code_writes_the_grant(pool: PgPool) {
    let discord = fake();
    let (application, user_code) = fixture(&pool).await;

    let response = approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["発行を許可しました。この申請は、すべての通貨を操作できます。"]
    );
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
    assert_eq!(request_status(&pool, application).await, "approved");
}

/// And an approval of a narrowed ask says what it was for: the currencies are
/// the ask's own, so a person who typed a code they were shown learns here what
/// the narrowing they just approved was.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_a_narrowed_ask_says_what_it_is_for(pool: PgPool) {
    let discord = fake();
    const CURRENCY: i64 = 7;

    insert_currency(&pool, CURRENCY, "nyan", "nyan", DEFAULT_GUILD, 500).await;

    let application = insert_application(&pool, FIRST_OWNER, "an application").await;
    let asked = vc_core::grant::request_grant(
        &pool,
        application,
        vc_core::grant::Target::Guild(DEFAULT_GUILD),
        &SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect::<Vec<_>>(),
        &[CURRENCY],
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("an ask");

    let response = approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(
            ADMIN,
            DEFAULT_PERMISSIONS,
            approve_options(&asked.user_code),
        ),
    )
    .await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["発行を許可しました。この申請は、通貨 nyan だけを操作できます。"]
    );
}

/// And the application it was written for is what the list then shows.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn approving_puts_the_application_on_the_list(pool: PgPool) {
    let discord = fake();
    let (application, user_code) = fixture(&pool).await;
    let client_id = client_id_of(&pool, application).await;

    approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&user_code)),
    )
    .await;

    let grant_id: i64 =
        sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE application_id = $1")
            .bind(application)
            .fetch_one(&pool)
            .await
            .unwrap();
    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(
        texts(&response)[1],
        format!(
            "Bot未連携: `an application`\nclient_id: `{client_id}`\nこの申請は、すべての通貨を操作できます。"
        )
    );
    assert_eq!(
        buttons(&response),
        [vc_api::custom_id::ui::grant::revoke_one_custom_id(grant_id)]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_code_that_names_nothing_pending_is_refused(pool: PgPool) {
    let discord = fake();
    fixture(&pool).await;

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options("deadbeef")),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "No pending request is available for you here. Server requests require an administrator in that server."
        ]
    );
}

/// An approval answers the ask's own scopes, never anything else: the grant the
/// device polls for is written from the request it made.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_grant_carries_the_asked_scopes(pool: PgPool) {
    let discord = fake();
    let (application, user_code) = fixture(&pool).await;

    approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool.clone()),
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
    let discord = fake();
    let (application, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;
    let notified = Arc::new(Recorded::default());

    let response = interaction(
        discord.clone(),
        router_with(discord.clone(), pool.clone(), notified.clone()),
        button_from_guild(
            json!({ "custom_id": revoke_button(&pool, &client_id).await }),
            ADMIN,
        ),
    )
    .await;

    assert_eq!(response.status, 202, "body: {}", response.body);
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
    let discord = fake();
    let (application, client_id) = authorized(&pool, FIRST_OWNER, "an application").await;

    let payload = json!({
        "type": 3,
        "data": { "custom_id": revoke_button(&pool, &client_id).await, "component_type": 2 },
        "member": { "user": { "id": ADMIN.to_string() }, "permissions": NOT_ADMIN },
        "guild_id": DEFAULT_GUILD.to_string(),
    });

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        payload,
    )
    .await;

    assert_eq!(response.status, 202, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        ["エラー: This grant is no longer available."]
    );
    assert!(allowed(&pool, application, DEFAULT_GUILD).await);
}

/// Five rows to a page and four arrows around them, the claim list's and contract
/// list's arithmetic: the sixth application is on page two, and the page a button
/// names is the page the redraw shows.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_list_pages(pool: PgPool) {
    let discord = fake();
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
        discord.clone(),
        router(discord.clone(), pool.clone()),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;

    assert_eq!(
        texts(&first)[0],
        "**発行を許可しているアプリケーション** (6件)\n\
         取り消すのは選んだ許可だけです。同じアプリケーションへの他の許可は残ります。"
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
        discord.clone(),
        router(discord.clone(), pool.clone()),
        button_from_guild(json!({ "custom_id": page_custom_id(Page::Next, 2) }), ADMIN),
    )
    .await;

    assert_eq!(second.body["type"], 7, "a redraw: {}", second.body);
    assert_eq!(
        texts(&second),
        [
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すのは選んだ許可だけです。同じアプリケーションへの他の許可は残ります。"
                .to_string(),
            format!(
                "Bot未連携: `an application 0`\nclient_id: `{}`\nこの申請は、すべての通貨を操作できます。",
                client_ids[0]
            ),
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
            discord.clone(),
            router(discord.clone(), pool.clone()),
            button_from_guild(json!({ "custom_id": arrow }), ADMIN),
        )
        .await;

        assert_eq!(back.status, 200, "body: {}", back.body);
        assert_eq!(back.body["type"], 7, "a redraw: {}", back.body);
        assert_eq!(
            texts(&back)[0],
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すのは選んだ許可だけです。同じアプリケーションへの他の許可は残ります。"
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
        discord.clone(),
        router(discord.clone(), pool),
        button_from_guild(json!({ "custom_id": buttons(&first)[8] }), ADMIN),
    )
    .await;

    assert_eq!(last.status, 200, "body: {}", last.body);
    assert_eq!(
        texts(&last),
        [
            "**発行を許可しているアプリケーション** (6件)\n\
             取り消すのは選んだ許可だけです。同じアプリケーションへの他の許可は残ります。"
                .to_string(),
            format!(
                "Bot未連携: `an application 0`\nclient_id: `{}`\nこの申請は、すべての通貨を操作できます。",
                client_ids[0]
            ),
        ]
    );
}

/// A decision is told to the application that asked, and the yes and the
/// taking-back travel the same way: a device that named a webhook learns what
/// its token may still do without polling for the difference.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_decision_pings_the_application(pool: PgPool) {
    let discord = fake();
    let (application, user_code) = fixture(&pool).await;
    let notified = Arc::new(Recorded::default());

    approve_and_confirm(
        discord.clone(),
        router_with(discord.clone(), pool.clone(), notified.clone()),
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
        discord.clone(),
        router_with(discord.clone(), pool.clone(), notified.clone()),
        button_from_guild(
            json!({ "custom_id": revoke_button(&pool, &client_id).await }),
            ADMIN,
        ),
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
    let discord = fake();
    let (_, user_code) = fixture(&pool).await;

    let response = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        grant_from_guild(ADMIN, NOT_ADMIN, approve_options(&user_code)),
    )
    .await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        texts(&response),
        [
            "No pending request is available for you here. Server requests require an administrator in that server."
        ]
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_command_is_refused_in_a_direct_message(pool: PgPool) {
    let discord = fake();
    fixture(&pool).await;

    let payload = json!({
        "type": 2,
        "data": { "name": "grant", "options": list_options() },
        "user": { "id": ADMIN.to_string() },
    });

    let response = interaction(discord.clone(), router(discord.clone(), pool), payload).await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(texts(&response), ["エラー: DMでは実行できません。"]);
}

async fn approve_and_confirm(
    discord: Arc<support::FakeDiscord>,
    app: Router,
    payload: Value,
) -> Response {
    let reviewed = interaction(discord.clone(), app.clone(), payload.clone()).await;
    let Some(id) = buttons(&reviewed).first().cloned() else {
        return reviewed;
    };
    let mut press = payload;
    press["type"] = json!(3);
    press["data"] = json!({"custom_id":id,"component_type":2});
    interaction(discord.clone(), app, press).await
}

fn from_dm(user: i64, options: Value) -> Value {
    json!({"type":2,"data":{"name":"grant","options":options},"user":{"id":user.to_string()}})
}
fn press_as(mut payload: Value, id: &str) -> Value {
    payload["type"] = json!(3);
    payload["data"] = json!({"component_type":2,"custom_id":id});
    payload
}
async fn personal_request(
    pool: &PgPool,
    app: i64,
    user: i64,
    scopes: &[&str],
) -> vc_core::grant::GrantRequest {
    vc_core::grant::request_grant(
        pool,
        app,
        vc_core::grant::Target::User(user),
        &scopes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
        &[],
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap()
}
async fn grant_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM grants")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn personal_review_confirmation_token_and_revocation(pool: PgPool) {
    let discord = fake();
    let app = insert_application(&pool, FIRST_OWNER, "personal app").await;
    // No preexisting user row: first consent must create a usable account.
    let asked = personal_request(
        &pool,
        app,
        ADMIN,
        &["vc.delegate.balances.read", "vc.delegate.claims.approve"],
    )
    .await;
    let notified = Arc::new(Recorded::default());
    let http = router_with(discord.clone(), pool.clone(), notified.clone());
    let command = from_dm(ADMIN, approve_options(&asked.user_code));
    let review = interaction(discord.clone(), http.clone(), command.clone()).await;
    let description = texts(&review).join("\n");
    assert!(description.contains("personal app"));
    assert!(description.contains("Your account"));
    assert!(description.contains("vc.delegate.claims.approve"));
    assert!(description.contains("pay from your account"));
    assert_eq!(grant_count(&pool).await, 0);
    assert!(notified.personal_grant_decisions().is_empty());
    let confirm = buttons(&review)[0].clone();
    assert!(confirm.chars().count() <= 100);
    // A copied button does not let another user consent, even as a guild admin.
    interaction(
        discord.clone(),
        http.clone(),
        press_as(
            grant_from_guild(ADMIN + 1, DEFAULT_PERMISSIONS, json!([])),
            &confirm,
        ),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 0);
    let (one, two) = tokio::join!(
        interaction(
            discord.clone(),
            http.clone(),
            press_as(command.clone(), &confirm)
        ),
        interaction(discord.clone(), http.clone(), press_as(command, &confirm))
    );
    assert_eq!(one.status, 202);
    assert_eq!(two.status, 202);
    assert_eq!(grant_count(&pool).await, 1);
    assert_eq!(notified.personal_grant_decisions().len(), 1);
    // Poll through the real token endpoint, then exercise the resulting account token.
    let (status, tokens) = poll_personal(&pool, app, &asked.device_code.to_string()).await;
    assert_eq!(status, 200);
    let token = tokens["access_token"].as_str().unwrap();
    assert_eq!(
        support::get(http.clone(), "/api/v2/users/@me/balances", Some(token))
            .await
            .status,
        200
    );
    let list = interaction(
        discord.clone(),
        http.clone(),
        from_dm(ADMIN, json!([{"name":"user","type":1}])),
    )
    .await;
    assert!(
        texts(&list)
            .join("\n")
            .contains("vc.delegate.balances.read")
    );
    let revoke = buttons(&list)[0].clone();
    interaction(
        discord.clone(),
        http.clone(),
        press_as(from_dm(ADMIN + 1, json!([])), &revoke),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 1);
    interaction(
        discord.clone(),
        http.clone(),
        press_as(from_dm(ADMIN, json!([])), &revoke),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 0);
    assert_eq!(
        support::get(http, "/api/v2/users/@me/balances", Some(token))
            .await
            .status,
        401
    );
    let refreshes: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refreshes, 0);
    assert_eq!(
        notified.personal_grant_decisions().last().unwrap().2,
        Vec::<String>::new()
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn personal_approval_in_a_guild_does_not_need_admin(pool: PgPool) {
    let discord = fake();
    let app = insert_application(&pool, FIRST_OWNER, "personal app").await;
    let asked = personal_request(&pool, app, ADMIN, &["vc.delegate.profile.read"]).await;
    approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        grant_from_guild(ADMIN, NOT_ADMIN, approve_options(&asked.user_code)),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 1);
    let other = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        from_dm(ADMIN + 1, approve_options(&asked.user_code)),
    )
    .await;
    assert!(buttons(&other).is_empty());
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn server_confirmation_rechecks_guild_permissions_and_expiry(pool: PgPool) {
    let discord = fake();
    let (_, code) = fixture(&pool).await;
    let http = router(discord.clone(), pool.clone());
    let payload = grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&code));
    let review = interaction(discord.clone(), http.clone(), payload.clone()).await;
    assert_eq!(grant_count(&pool).await, 0);
    assert!(texts(&review).join("\n").contains("This server"));
    let confirm = buttons(&review)[0].clone();
    interaction(
        discord.clone(),
        http.clone(),
        press_as(grant_from_guild(ADMIN, NOT_ADMIN, json!([])), &confirm),
    )
    .await;
    let mut wrong = payload.clone();
    wrong["guild_id"] = json!((DEFAULT_GUILD + 1).to_string());
    interaction(discord.clone(), http.clone(), press_as(wrong, &confirm)).await;
    interaction(
        discord.clone(),
        http.clone(),
        press_as(from_dm(ADMIN, json!([])), &confirm),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 0);
    sqlx::query("UPDATE grant_requests SET inserted_at=inserted_at-interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    interaction(discord.clone(), http, press_as(payload, &confirm)).await;
    assert_eq!(grant_count(&pool).await, 0);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_old_confirmation_cannot_approve_a_reused_code(pool: PgPool) {
    let discord = fake();
    let (app, code) = fixture(&pool).await;
    let http = router(discord.clone(), pool.clone());
    let payload = grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, approve_options(&code));
    let review = interaction(discord.clone(), http.clone(), payload.clone()).await;
    let confirm = buttons(&review)[0].clone();
    sqlx::query("UPDATE grant_requests SET inserted_at=inserted_at-interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    let replacement = vc_core::grant::request_grant(
        &pool,
        app,
        vc_core::grant::Target::Guild(DEFAULT_GUILD),
        &["vc.issue".to_owned()],
        &[],
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE grant_requests SET user_code=$1 WHERE id=$2")
        .bind(&code)
        .bind(replacement.id)
        .execute(&pool)
        .await
        .unwrap();
    interaction(discord.clone(), http, press_as(payload, &confirm)).await;
    assert_eq!(grant_count(&pool).await, 0);
    assert_eq!(request_status(&pool, app).await, "pending");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn pending_codes_are_global_and_random_collisions_retry(pool: PgPool) {
    let (_, code) = fixture(&pool).await;
    let app = insert_application(&pool, FIRST_OWNER, "second target").await;
    // Force the next generated code to collide once, even across target types.
    // Sequence increments survive rollback, so the second transaction can succeed.
    sqlx::raw_sql("CREATE SEQUENCE forced_collision; CREATE FUNCTION collide_once() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF nextval('forced_collision') = 1 THEN NEW.user_code := (SELECT user_code FROM grant_requests LIMIT 1); END IF; RETURN NEW; END $$; CREATE TRIGGER collide_once BEFORE INSERT ON grant_requests FOR EACH ROW EXECUTE FUNCTION collide_once();")
        .execute(&pool).await.unwrap();
    let asked = personal_request(&pool, app, ADMIN, &["vc.delegate.profile.read"]).await;
    assert_ne!(asked.user_code, code);
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM forced_collision")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(attempts, 2);
    let error = sqlx::query("UPDATE grant_requests SET user_code=$1 WHERE id=$2")
        .bind(code)
        .bind(asked.id)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().constraint(),
        Some("grant_requests_pending_user_code_index")
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn personal_list_paginates_and_approvals_remain_independent(pool: PgPool) {
    let discord = fake();
    let mut apps = Vec::new();
    for index in 0..5 {
        let app =
            insert_application(&pool, FIRST_OWNER + index, &format!("personal {index}")).await;
        let scopes: Vec<_> = vc_core::delegation::Scope::ALL
            .iter()
            .map(|scope| scope.as_str())
            .collect();
        let asked = personal_request(&pool, app, ADMIN, &scopes).await;
        approve_and_confirm(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            from_dm(ADMIN, approve_options(&asked.user_code)),
        )
        .await;
        apps.push(app);
    }
    let list = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        from_dm(ADMIN, json!([{"name":"user","type":1}])),
    )
    .await;
    assert!(texts(&list).join("\n").contains("Page 1/3"));
    let last = vc_api::custom_id::ui::grant::user_page_custom_id(ADMIN, 3);
    let last_page = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        press_as(from_dm(ADMIN, json!([])), &last),
    )
    .await;
    assert!(texts(&last_page).join("\n").contains("Page 3/3"));
    let requested = personal_request(&pool, apps[0], ADMIN, &["vc.delegate.profile.read"]).await;
    approve_and_confirm(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        from_dm(ADMIN, approve_options(&requested.user_code)),
    )
    .await;
    let scopes:Vec<String>=sqlx::query_scalar("SELECT s.scope::text FROM grant_scopes s JOIN grants g ON g.id=s.grant_id WHERE g.application_id=$1 AND g.discord_id=$2")
        .bind(apps[0]).bind(ADMIN).fetch_all(&pool).await.unwrap();
    assert_eq!(scopes.len(), vc_core::delegation::Scope::ALL.len() + 1);
    assert!(scopes.iter().any(|s| s == "vc.delegate.payments.create"));
    for app in &apps[1..] {
        let grant: i64 = sqlx::query_scalar("SELECT id FROM grants WHERE application_id = $1")
            .bind(app)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            vc_core::grant::revoke_one(&pool, grant, ADMIN, None)
                .await
                .unwrap()
        );
    }
    let clamped = interaction(
        discord.clone(),
        router(discord.clone(), pool),
        press_as(from_dm(ADMIN, json!([])), &last),
    )
    .await;
    assert!(texts(&clamped).join("\n").contains("Page 1/1"));
}

async fn poll_personal(pool: &PgPool, application: i64, device_code: &str) -> (u16, Value) {
    use base64::Engine;

    let client_id = support::client_id_of(pool, application).await;
    let client_secret = support::client_secret_of(pool, application).await;
    let basic =
        base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{client_secret}"));

    let response = axum::http::Request::builder()
        .method("POST")
        .uri("/oauth2/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", format!("Basic {basic}"))
        .body(axum::body::Body::from(format!(
            "grant_type=urn:ietf:params:oauth:grant-type:device_code&device_code={device_code}"
        )))
        .expect("request");

    let response = vc_api::router(state(pool.clone(), fake()))
        .oneshot(response)
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, serde_json::from_slice(&bytes).expect("json body"))
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn many_currencies_can_be_reviewed_approved_and_individually_revoked(pool: PgPool) {
    let discord = fake();
    use vc_api::custom_id::ui::grant as ids;
    let app = insert_application(&pool, FIRST_OWNER, "normal").await;
    let mut resources = Vec::new();
    let mut expected = Vec::new();
    for n in 1..=400i64 {
        let unit = format!(
            "aaaaaaaa{}{}",
            (b'a' + (n / 26) as u8) as char,
            (b'a' + (n % 26) as u8) as char
        );
        insert_currency(&pool, n, &unit, &unit, DEFAULT_GUILD + n, 1000).await;
        resources.push(n);
        expected.push(unit);
    }
    let scopes = vc_core::delegation::Scope::ALL
        .iter()
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    let asked = vc_core::grant::request_grant(
        &pool,
        app,
        vc_core::grant::Target::User(ADMIN),
        &scopes,
        &resources,
        600,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let http = router(discord.clone(), pool.clone());
    let first = interaction(
        discord.clone(),
        http.clone(),
        from_dm(ADMIN, approve_options(&asked.user_code)),
    )
    .await;
    let mut displayed = texts(&first).join("\n");
    let mut current = first;
    {
        let last = buttons(&current).last().cloned().unwrap();
        let ids::Pressed::ReviewPage(_, end) =
            ids::parse(&vc_api::custom_id::parse(&last)).unwrap()
        else {
            panic!("page button");
        };
        // The request has 400 ten-character entries, spread across five pages.
        for page in 2..=end {
            current = interaction(
                discord.clone(),
                http.clone(),
                press_as(
                    from_dm(ADMIN, json!([])),
                    &ids::review_page_custom_id(asked.id, page),
                ),
            )
            .await;
            displayed.push_str(&texts(&current).join("\n"));
        }
    }
    for unit in &expected {
        assert!(displayed.contains(unit), "missing {unit}");
    }
    let denied = interaction(
        discord.clone(),
        http.clone(),
        press_as(
            from_dm(ADMIN + 1, json!([])),
            &ids::review_page_custom_id(asked.id, 2),
        ),
    )
    .await;
    assert!(buttons(&denied).is_empty());
    interaction(
        discord.clone(),
        http.clone(),
        press_as(from_dm(ADMIN, json!([])), &ids::confirm_custom_id(asked.id)),
    )
    .await;
    let grant: i64 = sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE id = $1")
        .bind(asked.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let list = interaction(
        discord.clone(),
        http.clone(),
        from_dm(ADMIN, json!([{"name":"user","type":1}])),
    )
    .await;
    assert!(texts(&list).join("\n").contains("400 currencies"));
    assert!(buttons(&list).contains(&ids::details_custom_id(grant, 1)));
    let details = interaction(
        discord.clone(),
        http.clone(),
        press_as(
            from_dm(ADMIN, json!([])),
            &ids::details_custom_id(grant, i64::MAX),
        ),
    )
    .await;
    assert!(
        texts(&details)
            .join("\n")
            .contains(expected.last().unwrap())
    );
    let denied = interaction(
        discord.clone(),
        http.clone(),
        press_as(
            from_dm(ADMIN + 1, json!([])),
            &ids::details_custom_id(grant, 1),
        ),
    )
    .await;
    assert!(buttons(&denied).is_empty());
    let second = personal_request(&pool, app, ADMIN, &["vc.delegate.profile.read"]).await;
    approve_and_confirm(
        discord.clone(),
        http.clone(),
        from_dm(ADMIN, approve_options(&second.user_code)),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 2);
    interaction(
        discord.clone(),
        http,
        press_as(from_dm(ADMIN, json!([])), &ids::revoke_one_custom_id(grant)),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 1);
    assert_eq!(
        poll_personal(&pool, app, &asked.device_code.to_string())
            .await
            .0,
        400
    );
    assert_eq!(
        poll_personal(&pool, app, &second.device_code.to_string())
            .await
            .0,
        200
    );
}

async fn revoke_button(pool: &PgPool, client: &str) -> String {
    let grant: i64 = sqlx::query_scalar("SELECT g.id FROM grants g JOIN applications a ON a.id = g.application_id WHERE a.client_id::text = $1").bind(client).fetch_one(pool).await.unwrap();
    vc_api::custom_id::ui::grant::revoke_one_custom_id(grant)
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn server_grants_coexist_with_legacy_and_revoke_individually(pool: PgPool) {
    let discord = fake();
    use vc_api::custom_id::ui::grant as ids;
    let app = insert_application(&pool, FIRST_OWNER, "server app").await;
    insert_currency(&pool, 1, "nyan", "nyan", DEFAULT_GUILD, 100).await;
    let now = time::OffsetDateTime::now_utc();
    let legacy = vc_core::grant::grant_for_code(&pool, app, DEFAULT_GUILD, "legacy-code", now)
        .await
        .unwrap()
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    vc_core::grant::create_grant_scopes(&mut conn, legacy, &["vc.issue".to_owned()], now)
        .await
        .unwrap();
    let legacy_token = vc_core::grant::create_access_token(&mut *conn, legacy, now)
        .await
        .unwrap();
    drop(conn);
    let a = vc_core::grant::request_grant(
        &pool,
        app,
        vc_core::grant::Target::Guild(DEFAULT_GUILD),
        &["vc.issue".into()],
        &[],
        600,
        now,
    )
    .await
    .unwrap();
    let b = vc_core::grant::request_grant(
        &pool,
        app,
        vc_core::grant::Target::Guild(DEFAULT_GUILD),
        &["vc.issue".into()],
        &[1],
        600,
        now,
    )
    .await
    .unwrap();
    assert_ne!(a.id, b.id);
    let http = router(discord.clone(), pool.clone());
    for asked in [&a, &b] {
        approve_and_confirm(
            discord.clone(),
            http.clone(),
            grant_from_guild(
                ADMIN,
                DEFAULT_PERMISSIONS,
                approve_options(&asked.user_code),
            ),
        )
        .await;
    }
    let ga: i64 = sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE id = $1")
        .bind(a.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let gb: i64 = sqlx::query_scalar("SELECT grant_id FROM grant_requests WHERE id = $1")
        .bind(b.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(ga, gb);
    assert_ne!(legacy, ga);
    assert_ne!(legacy, gb);
    let ta = vc_core::grant::create_device_token(&pool, &a, now)
        .await
        .unwrap()
        .unwrap()
        .access_token;
    let tb = vc_core::grant::create_device_token(&pool, &b, now)
        .await
        .unwrap()
        .unwrap()
        .access_token;
    for (token, resources) in [(&legacy_token, vec![]), (&ta, vec![]), (&tb, vec![1])] {
        assert_eq!(
            vc_core::grant::resolve_token(&pool, token.parse().unwrap(), now)
                .await
                .unwrap()
                .unwrap()
                .resources,
            resources
        );
    }
    // A browser authorization-code exchange still targets its legacy row.
    assert_eq!(
        vc_core::grant::grant_for_code(&pool, app, DEFAULT_GUILD, "next-legacy-code", now)
            .await
            .unwrap(),
        Some(legacy)
    );
    let list = interaction(
        discord.clone(),
        http.clone(),
        grant_from_guild(ADMIN, DEFAULT_PERMISSIONS, list_options()),
    )
    .await;
    for id in [legacy, ga, gb] {
        assert!(buttons(&list).contains(&ids::revoke_one_custom_id(id)));
    }
    let revoke = ids::revoke_one_custom_id(ga);
    let mut other_guild = button_from_guild(json!({"custom_id":revoke}), ADMIN);
    other_guild["guild_id"] = json!((DEFAULT_GUILD + 1).to_string());
    interaction(discord.clone(), http.clone(), other_guild).await;
    interaction(
        discord.clone(),
        http.clone(),
        press_as(from_dm(ADMIN, json!([])), &revoke),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 3);
    interaction(
        discord.clone(),
        http.clone(),
        button_from_guild(json!({"custom_id":revoke}), ADMIN),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 2);
    assert!(
        vc_core::grant::resolve_token(&pool, ta.parse().unwrap(), now)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        vc_core::grant::create_device_token(&pool, &a, now)
            .await
            .unwrap()
            .is_none()
    );
    for token in [legacy_token, tb] {
        assert!(
            vc_core::grant::resolve_token(&pool, token.parse().unwrap(), now)
                .await
                .unwrap()
                .is_some()
        );
    }
    // A stale application-wide button must never revoke a later independent grant.
    let client = client_id_of(&pool, app).await;
    interaction(
        discord.clone(),
        http,
        button_from_guild(json!({"custom_id":ids::revoke_custom_id(&client)}), ADMIN),
    )
    .await;
    assert_eq!(grant_count(&pool).await, 2);
}
