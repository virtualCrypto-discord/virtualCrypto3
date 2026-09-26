//! `/mute`: what a person has chosen not to see.
//!
//! An addition rather than a port — the Elixir has no mute, no column that could hold one, and
//! nothing that filters a list by its reader — so nothing here has an Elixir case behind it:
//! `docs/known-gaps.md` is the design, and this file is what holds it to the three things that
//! file claims. Nothing is forbidden, a page and its count agree, and a single row is not
//! filtered.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    MONEY_USER1, MONEY_USER2, Money, Response, account_of, button_from_guild, execute_from_dm,
    fake, get, insert_application, insert_claim, insert_user, mint, mint_app,
    rendered_interaction as interaction, setup_money, state,
};
use tower::ServiceExt;
use vc_core::claim::SrFilter;

/// Somebody the money fixture does not have: a second person claiming against the same payer, so
/// that a mute can take away part of a list rather than all of it.
const OTHER: i32 = 3;
const OTHER_DISCORD_ID: i64 = 100_000_000_000_000_003;

/// The account the fixture's second Discord user has, which is who mutes: every list in this
/// file is read from their side.
const MUTER: i32 = 2;

fn router(discord: std::sync::Arc<support::FakeDiscord>, pool: PgPool) -> Router {
    vc_api::router(state(pool, discord))
}

/// A `/mute` or `/unmute` interaction, typed in a direct message by the person it belongs to:
/// both commands are personal, so there is no guild and no administrator anywhere in these.
fn command(name: &str, user: i64, subcommand: &str, option: Option<(&str, Value)>) -> Value {
    let options: Vec<Value> = match option {
        Some((name, value)) => vec![json!({ "name": name, "value": value })],
        None => Vec::new(),
    };

    execute_from_dm(
        json!({
            "name": name,
            "options": [{ "name": subcommand, "type": 1, "options": options }],
        }),
        user,
    )
}

fn mute(user: i64, subcommand: &str, option: Option<(&str, Value)>) -> Value {
    command("mute", user, subcommand, option)
}

fn unmute(user: i64, subcommand: &str, option: Option<(&str, Value)>) -> Value {
    command("unmute", user, subcommand, option)
}

/// `/claim list`, as the command opens it: pending claims, first page, both positions.
fn claim_list(user: i64) -> Value {
    execute_from_dm(
        json!({ "name": "claim", "options": [{ "name": "list", "options": [] }] }),
        user,
    )
}

/// `/contract list`, which is the other list a mute filters.
fn contract_list(user: i64) -> Value {
    execute_from_dm(
        json!({ "name": "contract", "options": [{ "name": "list", "options": [] }] }),
        user,
    )
}

/// Everything one answer says, as one string: what a person reads, without the shape it arrived
/// in, which is what a test about a sentence wants.
fn rendered(response: &Response) -> String {
    response.body["data"].to_string()
}

/// The custom id of the first accessory button in an answer, which is a row's own 解除.
fn accessory(response: &Response) -> String {
    response.body["data"]["components"][0]["components"]
        .as_array()
        .expect("the container's children")
        .iter()
        .find_map(|child| child["accessory"]["custom_id"].as_str())
        .expect("a row with a button")
        .to_owned()
}

/// Two claims on the same payer, one in each of the fixture's currencies, and the fixture they
/// were made in.
///
/// The amount is what tells them apart on a screen: `/claim list` names the other party, who is
/// the same person in both, and then the currency's unit.
async fn two_currencies_claimed(pool: &PgPool) -> Money {
    let money = setup_money(pool).await;

    insert_claim(pool, 1, 500, "pending", 1, MUTER, money.currency).await;
    insert_claim(pool, 2, 700, "pending", 1, MUTER, money.currency2).await;

    money
}

/// A currency's mute leaves its claims out of the list, and the claim in the other currency
/// stays — which is the whole of what 「表示しない」 means.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_muted_currency_leaves_the_claim_list(pool: PgPool) {
    let discord = fake();
    let money = two_currencies_claimed(&pool).await;

    let before = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            claim_list(MONEY_USER2),
        )
        .await,
    );
    assert!(before.contains("請求額: **500**"), "{before}");
    assert!(before.contains("請求額: **700**"), "{before}");

    let muted = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;
    assert_eq!(muted.status, 202, "body: {}", muted.body);

    let after = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            claim_list(MONEY_USER2),
        )
        .await,
    );
    assert!(!after.contains("請求額: **500**"), "{after}");
    assert!(after.contains("請求額: **700**"), "{after}");
}

/// The same for a person: muting somebody hides the claims between you and them, in every
/// currency, and leaves everybody else's alone.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_muted_person_leaves_the_claim_list(pool: PgPool) {
    let discord = fake();
    let money = setup_money(&pool).await;
    insert_user(&pool, OTHER, OTHER_DISCORD_ID).await;

    insert_claim(&pool, 1, 500, "pending", 1, MUTER, money.currency).await;
    insert_claim(&pool, 2, 700, "pending", OTHER, MUTER, money.currency).await;

    let muted = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(
            MONEY_USER2,
            "user",
            Some(("user", json!(MONEY_USER1.to_string()))),
        ),
    )
    .await;
    assert_eq!(muted.status, 202, "body: {}", muted.body);

    let after = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            claim_list(MONEY_USER2),
        )
        .await,
    );
    assert!(!after.contains("請求額: **500**"), "{after}");
    assert!(after.contains("請求額: **700**"), "{after}");
}

/// The API answers the same list as the screens, because it is the same reader: a muted currency
/// is left out of `/users/@me/claims` too, and comes back when the mute does.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_api_answers_the_list_the_reader_can_see(pool: PgPool) {
    let discord = fake();
    let money = two_currencies_claimed(&pool).await;
    let token = mint(&pool, MUTER, &["vc.claim"]).await;

    let before = get(
        router(discord.clone(), pool.clone()),
        "/api/v2/users/@me/claims",
        Some(&token),
    )
    .await;
    assert_eq!(before.status, 200, "body: {}", before.body);
    assert_eq!(before.body.as_array().expect("a list").len(), 2);

    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;

    let muting = get(
        router(discord.clone(), pool.clone()),
        "/api/v2/users/@me/claims",
        Some(&token),
    )
    .await;
    let rows = muting.body.as_array().expect("a list").to_vec();
    assert_eq!(rows.len(), 1, "body: {}", muting.body);
    assert_eq!(rows[0]["amount"], "700");

    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        unmute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;

    let after = get(
        router(discord.clone(), pool.clone()),
        "/api/v2/users/@me/claims",
        Some(&token),
    )
    .await;
    assert_eq!(after.body.as_array().expect("a list").len(), 2);
}

/// `:last` is counted the way the page is read. Six of the twelve claims belong to a muted
/// person, so the last page is the second — and a count taken over the table's twelve would send
/// the button to a third page with nothing on it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_last_page_counts_only_what_the_reader_can_see(pool: PgPool) {
    let discord = fake();
    let money = setup_money(&pool).await;
    insert_user(&pool, OTHER, OTHER_DISCORD_ID).await;

    for id in 1..=6 {
        insert_claim(&pool, id, 100 + id, "pending", 1, MUTER, money.currency).await;
    }

    for id in 7..=12 {
        insert_claim(&pool, id, 100 + id, "pending", OTHER, MUTER, money.currency).await;
    }

    let statuses = vec!["pending".to_string()];

    let before = vc_core::claim::last_page_number(&pool, MUTER, &statuses, SrFilter::All, None, 5)
        .await
        .expect("the count");
    assert_eq!(before, 3);

    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(
            MONEY_USER2,
            "user",
            Some(("user", json!(MONEY_USER1.to_string()))),
        ),
    )
    .await;

    let after = vc_core::claim::last_page_number(&pool, MUTER, &statuses, SrFilter::All, None, 5)
        .await
        .expect("the count");
    assert_eq!(after, 2);

    // And what the last page holds is the sixth of the rows that are left: newest first, so the
    // lowest id of the six that are visible.
    let last = vc_core::claim::list_page(&pool, MUTER, &statuses, SrFilter::All, None, 2, 5)
        .await
        .expect("the last page");
    assert_eq!(last.claims.len(), 1);
    assert_eq!(last.claims[0].amount, Some(107));
}

/// The screen lists what is muted, and the button on a row is what takes it back — the round
/// trip a person makes, in the order they make it.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_mute_screen_shows_its_rows_and_takes_one_back(pool: PgPool) {
    let discord = fake();
    let money = setup_money(&pool).await;
    insert_user(&pool, OTHER, OTHER_DISCORD_ID).await;

    // The person first and the currency second, because the list is newest first and the row a
    // press takes back is the one the screen put at the top.
    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(
            MONEY_USER2,
            "user",
            Some(("user", json!(OTHER_DISCORD_ID.to_string()))),
        ),
    )
    .await;
    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;

    let screen = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "list", None),
    )
    .await;
    let shown = rendered(&screen);
    assert!(shown.contains("**ミュート一覧** (2件)"), "{shown}");
    assert!(shown.contains(&money.name), "{shown}");
    assert!(shown.contains(&OTHER_DISCORD_ID.to_string()), "{shown}");

    // A second mute of the same currency is the mute that is already there, and says so.
    let again = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;
    assert!(
        rendered(&again).contains("すでにミュートしています"),
        "{}",
        again.body
    );

    // The row's own button, taken from the screen rather than rebuilt: what a person presses is
    // what the screen drew.
    let pressed = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        button_from_guild(json!({ "custom_id": accessory(&screen) }), MONEY_USER2),
    )
    .await;
    assert_eq!(pressed.status, 202, "body: {}", pressed.body);

    let after = rendered(&pressed);
    assert!(after.contains("**ミュート一覧** (1件)"), "{after}");
    assert!(!after.contains(&money.name), "{after}");

    // And the person is still muted, which is what the list would have said a press too many.
    assert!(after.contains(&OTHER_DISCORD_ID.to_string()), "{after}");
}

/// The list is the reader's own: what somebody else muted changes nothing about yours.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn one_persons_mute_is_not_another_persons(pool: PgPool) {
    let discord = fake();
    let money = two_currencies_claimed(&pool).await;

    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;

    // The claimant's own list, which is the same claim seen from the other side.
    let theirs = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            claim_list(MONEY_USER1),
        )
        .await,
    );
    assert!(theirs.contains("請求額: **500**"), "{theirs}");
}

/// Yourself: your own actions are what a list is about, so the mute is refused rather than
/// written as a row that would leave you looking at nothing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn muting_yourself_is_refused(pool: PgPool) {
    let discord = fake();
    setup_money(&pool).await;

    let refused = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(
            MONEY_USER2,
            "user",
            Some(("user", json!(MONEY_USER2.to_string()))),
        ),
    )
    .await;

    assert_eq!(refused.status, 202, "body: {}", refused.body);
    assert!(
        rendered(&refused).contains("自分自身はミュートできません"),
        "{}",
        refused.body
    );
}

/// Somebody Discord knows and this service does not: nothing of theirs can be in a list yet, so
/// the answer says what is missing rather than refusing.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn muting_somebody_with_no_account_says_so(pool: PgPool) {
    let discord = fake();
    setup_money(&pool).await;

    let stranger: i64 = 100_000_000_000_000_009;
    let answered = interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(
            MONEY_USER2,
            "user",
            Some(("user", json!(stranger.to_string()))),
        ),
    )
    .await;

    assert_eq!(answered.status, 202, "body: {}", answered.body);
    assert!(
        rendered(&answered).contains("アカウントがまだありません"),
        "{}",
        answered.body
    );
}

/// The contract list, which is the other list a mute filters, and the one that says its count
/// out loud — so a page and its count agreeing is something a person can read.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_muted_currency_leaves_the_contract_list(pool: PgPool) {
    let discord = fake();
    let money = setup_money(&pool).await;

    // An application contracts for both currencies, naming the muter as the party: what a
    // `/contract list` from them has to show, and then has to hide half of.
    let application = insert_application(&pool, MONEY_USER1, "a service").await;
    let token = mint_app(
        &pool,
        account_of(&pool, application).await,
        &["vc.contract"],
    )
    .await;

    for unit in [&money.unit, &money.unit2] {
        let created = send(
            router(discord.clone(), pool.clone()),
            "/api/v2/contracts",
            &token,
            json!({
                "unit": unit,
                "parties": [{ "discord_id": MONEY_USER2.to_string(), "amount": "100" }],
            }),
        )
        .await;

        assert_eq!(created.status, 201, "body: {}", created.body);
    }

    let before = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            contract_list(MONEY_USER2),
        )
        .await,
    );
    assert!(before.contains("**契約** (2件)"), "{before}");

    interaction(
        discord.clone(),
        router(discord.clone(), pool.clone()),
        mute(MONEY_USER2, "currency", Some(("unit", json!(&money.unit)))),
    )
    .await;

    let after = rendered(
        &interaction(
            discord.clone(),
            router(discord.clone(), pool.clone()),
            contract_list(MONEY_USER2),
        )
        .await,
    );
    assert!(after.contains("**契約** (1件)"), "{after}");
    assert!(after.contains(&format!("（{}）", money.unit2)), "{after}");

    // The API answers the same list, as it does for claims.
    let listed = get(
        router(discord.clone(), pool.clone()),
        "/api/v2/users/@me/contracts",
        Some(&mint(&pool, MUTER, &[]).await),
    )
    .await;
    assert_eq!(
        listed.body.as_array().expect("a list").len(),
        1,
        "body: {}",
        listed.body
    );
}

/// A `POST` with a body, which the shared support has no shape for: the claim and contract
/// suites each keep their own, and this is that one, for the calls an application makes.
async fn send(app: Router, uri: &str, token: &str, body: Value) -> Response {
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("a request"),
        )
        .await
        .expect("a response");

    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body");

    Response {
        status,
        headers,
        body: if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a json body")
        },
    }
}
