//! RFC 8707's `resource`: the currencies a grant is for, and what a token the
//! grant hands out may then touch.
//!
//! `docs/resources.md` is the design and `crates/vc-api/src/resource.rs` is the
//! one function the endpoints share. What these pin is the difference it draws:
//! an act that moves money in a currency outside the grant is `403
//! insufficient_scope`, a read that *names* one is refused the same way, and the
//! two lists are *filtered* rather than refused. A credential that is the
//! account's own — a personal access token, an application's `client_credentials`
//! token — is not a grant and is not narrowed at all.

mod support;

use axum::Router;
use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    Response, account_of, fake, get, insert_application, insert_asset, insert_claim,
    insert_currency, insert_grant_for, insert_personal_grant, insert_user, mint, mint_app, state,
};
use tower::ServiceExt;
use vc_core::grant::Target;

/// Two guilds with a currency each, and a third a test can create a currency in
/// later. The person holds both currencies; the receiver is who a payment is to.
const GUILD_A: i64 = 900_000_000_000_000_001;
const GUILD_B: i64 = 900_000_000_000_000_002;
const GUILD_C: i64 = 900_000_000_000_000_003;
const CURRENCY_A: i64 = 1;
const CURRENCY_B: i64 = 2;
const CURRENCY_C: i64 = 3;
const UNIT_A: &str = "nyan";
const UNIT_B: &str = "wan";
const UNIT_C: &str = "kaguya";

/// The site a `resource` URI is built from, which is the state's own.
const SITE: &str = "https://vcrypto.sumidora.com";

const PERSON_ACCOUNT: i32 = 1;
const PERSON: i64 = 700_000_000_000_000_001;
const RECEIVER_ACCOUNT: i32 = 2;
const RECEIVER: i64 = 700_000_000_000_000_002;

/// The owner of the applications these tests grant things to; a Discord id that
/// is nobody's account, as the application fixtures' owner always is.
const OWNER: i64 = 900_000_000_000_000_010;

fn router(pool: PgPool) -> Router {
    vc_api::router(state(pool, fake()))
}

/// The person, who holds both currencies, and a receiver to pay.
async fn fixture(pool: &PgPool) {
    insert_user(pool, PERSON_ACCOUNT, PERSON).await;
    insert_user(pool, RECEIVER_ACCOUNT, RECEIVER).await;
    insert_currency(pool, CURRENCY_A, "nyan", UNIT_A, GUILD_A, 500).await;
    insert_currency(pool, CURRENCY_B, "wan", UNIT_B, GUILD_B, 500).await;
    insert_asset(pool, PERSON_ACCOUNT, CURRENCY_A, 1_000).await;
    insert_asset(pool, PERSON_ACCOUNT, CURRENCY_B, 1_000).await;
}

/// An HTTP call with a JSON body, answered as its status, headers and parsed
/// body.
async fn request(
    app: Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Value,
) -> Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json");

    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }

    if !body.is_null() {
        builder = builder.header("content-type", "application/json");
    }

    let request = builder
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("encode body"),
        ))
        .expect("request");

    let response = app.oneshot(request).await.expect("router response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json body")
    };

    Response {
        status,
        headers,
        body,
    }
}

/// `POST /oauth2/clients/@me/grant-requests`, the ask both flows narrow.
async fn ask(pool: &PgPool, token: &str, body: Value) -> Response {
    request(
        router(pool.clone()),
        "POST",
        "/oauth2/clients/@me/grant-requests",
        Some(token),
        body,
    )
    .await
}

/// The device poll, exactly as `tests/grant_requests.rs` makes it: basic auth
/// with the application's own id and secret.
async fn device_poll(pool: &PgPool, application: i64, device_code: &str) -> (u16, Value) {
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

    let response = router(pool.clone())
        .oneshot(response)
        .await
        .expect("router response");

    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");

    (status, serde_json::from_slice(&bytes).expect("json body"))
}

/// The application's own token, which is what asks: `mint_app` is the
/// registration token's issuance, `kind: app` with `oauth2.register`.
async fn application_token(pool: &PgPool) -> (i64, String) {
    let application = insert_application(pool, OWNER, "an application").await;
    let account = account_of(pool, application).await;
    let token = mint_app(pool, account, &["oauth2.register"]).await;

    (application, token)
}

/// An application asks a person through the API — so the `resource` field is
/// resolved the way production resolves it — the person approves, and the poll
/// answers with the token the grant hands out. Answers the application, the
/// token, and the currencies the grant ended up narrowed to.
async fn ask_and_approve(
    pool: &PgPool,
    resource: Option<Value>,
    scopes: &[&str],
) -> (i64, String, Vec<i64>) {
    let (application, token) = application_token(pool).await;

    let mut body = json!({ "discord_id": PERSON.to_string(), "scopes": scopes });
    if let Some(resource) = resource {
        body["resource"] = resource;
    }

    let asked = ask(pool, &token, body).await;
    assert_eq!(asked.status, 201, "body: {}", asked.body);

    let user_code = asked.body["user_code"].as_str().expect("a user code");
    let device_code = asked.body["device_code"].as_str().expect("a device code");

    vc_core::grant::decide_request(
        pool,
        user_code,
        Target::User(PERSON),
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("the decision")
    .expect("the person answered");

    let (status, body) = device_poll(pool, application, device_code).await;
    assert_eq!(status, 200, "body: {body}");
    let token = body["access_token"].as_str().expect("a token").to_owned();

    let resources = sqlx::query_scalar!(
        r#"SELECT COALESCE((SELECT array_agg(r.currency_id) FROM grant_resources r
                              WHERE r.grant_id = g.id), ARRAY[]::bigint[]) AS "resources!"
             FROM grants g
            WHERE g.application_id = $1 AND g.discord_id = $2"#,
        application,
        PERSON
    )
    .fetch_one(pool)
    .await
    .expect("the grant's resources");

    (application, token, resources)
}

/// `403 insufficient_scope`, which is one answer for a scope the token lacks and
/// a currency the grant does not name: the token is real, and it is for
/// something else.
fn insufficient_scope() -> Value {
    json!({ "error": "insufficient_scope", "error_description": "token_verification_failed" })
}

fn pay(unit: &str, amount: &str) -> Value {
    json!({
        "unit": unit,
        "receiver_discord_id": RECEIVER.to_string(),
        "amount": amount,
    })
}

// --- what a narrowed grant may do ------------------------------------------

/// A grant narrowed to one currency refuses the money-moving acts that land on
/// another, and allows the ones that land on the named one. Paying names its
/// currency by unit; issuing lands on the guild's own.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_narrowed_grant_refuses_an_act_in_another_currency(pool: PgPool) {
    fixture(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;

    // A person's grant, narrowed to nyan: it may pay in nyan and nothing else.
    let personal =
        insert_personal_grant(&pool, application, PERSON, &["vc.pay"], &[CURRENCY_A]).await;

    let refused = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&personal),
        pay(UNIT_B, "10"),
    )
    .await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(refused.body, insufficient_scope());

    let paid = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&personal),
        pay(UNIT_A, "10"),
    )
    .await;
    assert_eq!(paid.status, 201, "body: {}", paid.body);

    // A guild's grant, narrowed to a currency that is not the guild's own: the
    // issue it would make lands on the guild's currency, which the grant does
    // not name. (The ask cannot write this — a guild may only name its own
    // currency — so it is built here to exercise the check on the row.)
    let narrowed_away =
        insert_grant_for(&pool, application, GUILD_B, &["vc.issue"], &[CURRENCY_A]).await;

    let refused = request(
        router(pool.clone()),
        "POST",
        "/api/v2/currencies/issue",
        Some(&narrowed_away),
        json!({ "receiver_discord_id": RECEIVER.to_string(), "amount": "10" }),
    )
    .await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(refused.body, insufficient_scope());

    // The same, narrowed to the guild's own currency, issues.
    let named = insert_grant_for(&pool, application, GUILD_A, &["vc.issue"], &[CURRENCY_A]).await;

    let issued = request(
        router(pool.clone()),
        "POST",
        "/api/v2/currencies/issue",
        Some(&named),
        json!({ "receiver_discord_id": RECEIVER.to_string(), "amount": "10" }),
    )
    .await;
    assert_eq!(issued.status, 201, "body: {}", issued.body);
    assert_eq!(issued.body["unit"], UNIT_A);
}

/// The collection URI is how a client that cannot know its currencies asks for
/// all of them, and it says the same thing as naming none: every currency of the
/// account, now and later. A currency the guild creates *after* the grant is one
/// a set frozen at the ask would have left out.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_collection_form_covers_a_currency_created_later(pool: PgPool) {
    fixture(&pool).await;

    let (_, token, resources) = ask_and_approve(
        &pool,
        Some(json!([format!("{SITE}/api/v2/currencies")])),
        &["vc.pay"],
    )
    .await;
    assert!(
        resources.is_empty(),
        "the collection writes no row, so it means every currency: {resources:?}"
    );

    insert_currency(&pool, CURRENCY_C, "kaguya", UNIT_C, GUILD_C, 500).await;
    insert_asset(&pool, PERSON_ACCOUNT, CURRENCY_C, 1_000).await;

    let paid = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_C, "10"),
    )
    .await;

    assert_eq!(paid.status, 201, "body: {}", paid.body);
}

/// The same fact written the other way: an ask that names no currency at all.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_omitted_form_covers_a_currency_created_later(pool: PgPool) {
    fixture(&pool).await;

    let (_, token, resources) = ask_and_approve(&pool, None, &["vc.pay"]).await;
    assert!(
        resources.is_empty(),
        "naming none means every currency: {resources:?}"
    );

    insert_currency(&pool, CURRENCY_C, "kaguya", UNIT_C, GUILD_C, 500).await;
    insert_asset(&pool, PERSON_ACCOUNT, CURRENCY_C, 1_000).await;

    let paid = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_C, "10"),
    )
    .await;

    assert_eq!(paid.status, 201, "body: {}", paid.body);
}

// --- what is not a resource of this service --------------------------------

/// A `resource` that is not absolute, not this service's, not a currency, or —
/// for a guild's ask — a currency of another guild is `invalid_target`, RFC
/// 8707's own error.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_resource_that_is_not_one_is_an_invalid_target(pool: PgPool) {
    fixture(&pool).await;
    let (_, token) = application_token(&pool).await;

    let guild_ask = |resource: Value| {
        let token = token.clone();
        let pool = pool.clone();

        async move {
            ask(
                &pool,
                &token,
                json!({
                    "guild_id": GUILD_A.to_string(),
                    "scopes": ["vc.issue"],
                    "resource": resource,
                }),
            )
            .await
        }
    };

    // Not this service's own origin.
    let foreign = guild_ask(json!(["https://elsewhere.example/api/v2/currencies/1"])).await;
    assert_eq!(foreign.status, 400, "body: {}", foreign.body);
    assert_eq!(
        foreign.body,
        json!({ "error": "invalid_target", "error_description": "invalid_target" })
    );

    // This service's, but no currency has that id.
    let unknown = guild_ask(json!([format!("{SITE}/api/v2/currencies/999")])).await;
    assert_eq!(unknown.status, 400, "body: {}", unknown.body);
    assert_eq!(
        unknown.body,
        json!({ "error": "invalid_target", "error_description": "invalid_target" })
    );

    // A currency that exists, but of a guild this ask is not put to.
    let other_guild = guild_ask(json!([format!("{SITE}/api/v2/currencies/{CURRENCY_B}")])).await;
    assert_eq!(other_guild.status, 400, "body: {}", other_guild.body);
    assert_eq!(
        other_guild.body,
        json!({ "error": "invalid_target", "error_description": "invalid_target" })
    );
}

// --- what the reads answer --------------------------------------------------

/// The lists are *filtered* to the grant's currencies rather than refused: a
/// holding or a claim the token is not for is one the answer does not mention.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn the_lists_are_filtered_to_the_grants_currencies(pool: PgPool) {
    fixture(&pool).await;
    insert_claim(
        &pool,
        1,
        500,
        "pending",
        PERSON_ACCOUNT,
        RECEIVER_ACCOUNT,
        CURRENCY_A,
    )
    .await;
    insert_claim(
        &pool,
        2,
        500,
        "pending",
        PERSON_ACCOUNT,
        RECEIVER_ACCOUNT,
        CURRENCY_B,
    )
    .await;

    let application = insert_application(&pool, OWNER, "an application").await;
    let token = insert_personal_grant(
        &pool,
        application,
        PERSON,
        &["vc.read", "vc.claim"],
        &[CURRENCY_A],
    )
    .await;

    let balances = get(
        router(pool.clone()),
        "/api/v2/users/@me/balances",
        Some(&token),
    )
    .await;
    assert_eq!(balances.status, 200, "body: {}", balances.body);
    let units: Vec<&str> = balances
        .body
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|holding| holding["currency"]["unit"].as_str())
        .collect();
    assert_eq!(units, [UNIT_A], "body: {}", balances.body);

    let claims = get(
        router(pool.clone()),
        "/api/v2/users/@me/claims",
        Some(&token),
    )
    .await;
    assert_eq!(claims.status, 200, "body: {}", claims.body);
    let ids: Vec<&str> = claims
        .body
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|claim| claim["id"].as_str())
        .collect();
    assert_eq!(ids, ["1"], "body: {}", claims.body);
}

/// A read that *names* a currency is refused like a write, because the request
/// asked for something the token is not worth. The named one is answered.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_named_currency_read_is_refused_outside_the_grant(pool: PgPool) {
    fixture(&pool).await;
    let application = insert_application(&pool, OWNER, "an application").await;
    let token =
        insert_personal_grant(&pool, application, PERSON, &["vc.read"], &[CURRENCY_A]).await;

    let refused = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{CURRENCY_B}"),
        Some(&token),
    )
    .await;
    assert_eq!(refused.status, 403, "body: {}", refused.body);
    assert_eq!(refused.body, insufficient_scope());

    let named = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{CURRENCY_A}"),
        Some(&token),
    )
    .await;
    assert_eq!(named.status, 200, "body: {}", named.body);
    assert_eq!(named.body["unit"], UNIT_A);
}

// --- what is not a grant at all --------------------------------------------

/// A personal access token and an application's own `client_credentials` token
/// are the account's own credentials, not a delegation, so the currencies a
/// grant names say nothing about what they may do.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_accounts_own_credential_is_not_narrowed(pool: PgPool) {
    fixture(&pool).await;

    // A PAT is a `kind: user` token, which is what `mint` issues.
    let pat = mint(&pool, PERSON_ACCOUNT, &["vc.pay"]).await;

    let paid = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&pat),
        pay(UNIT_B, "10"),
    )
    .await;
    assert_eq!(paid.status, 201, "body: {}", paid.body);

    // And the application's own token, `kind: app` from `client_credentials`,
    // reads a currency no grant of it names.
    let (application, _) = application_token(&pool).await;
    let credentials = mint_app(&pool, account_of(&pool, application).await, &["vc.pay"]).await;

    let read = get(
        router(pool.clone()),
        &format!("/api/v2/currencies/{CURRENCY_B}"),
        Some(&credentials),
    )
    .await;
    assert_eq!(read.status, 200, "body: {}", read.body);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deleting_a_currency_does_not_widen_its_grant(pool: PgPool) {
    fixture(&pool).await;
    let (_, token, resources) = ask_and_approve(
        &pool,
        Some(json!([format!("{SITE}/api/v2/currencies/{CURRENCY_A}")])),
        &["vc.pay"],
    )
    .await;
    assert_eq!(resources, vec![CURRENCY_A]);
    let before = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_B, "100"),
    )
    .await;
    assert_eq!(before.status, 403);
    assert_eq!(
        vc_core::currency::delete(&pool, GUILD_A, &format!("delete {UNIT_A}"))
            .await
            .unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let after = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_B, "100"),
    )
    .await;
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2")
            .bind(i64::from(PERSON_ACCOUNT))
            .bind(CURRENCY_B)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after.status, 403,
        "after deleting allowed currency A, payment in unapproved B returned {}; B balance is {balance} (was 1000)",
        after.status
    );
    assert_eq!(balance, 1000);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn deleting_one_resource_preserves_the_remaining_permission(pool: PgPool) {
    fixture(&pool).await;
    let (_, token, _) = ask_and_approve(
        &pool,
        Some(json!([
            format!("{SITE}/api/v2/currencies/{CURRENCY_A}"),
            format!("{SITE}/api/v2/currencies/{CURRENCY_B}")
        ])),
        &["vc.pay"],
    )
    .await;
    assert_eq!(
        vc_core::currency::delete(&pool, GUILD_A, &format!("delete {UNIT_A}"))
            .await
            .unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    let allowed = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_B, "100"),
    )
    .await;
    assert_eq!(allowed.status, 201);
    // Reusing the guild and unit gives a new currency id, not the old permission.
    vc_core::currency::create(&pool, GUILD_A, "replacement", UNIT_A, PERSON, 1000)
        .await
        .unwrap();
    let refused = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_A, "100"),
    )
    .await;
    assert_eq!(refused.status, 403);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn unrestricted_grants_still_cover_replacement_currencies(pool: PgPool) {
    fixture(&pool).await;
    let (_, token, _) = ask_and_approve(&pool, None, &["vc.pay"]).await;
    assert_eq!(
        vc_core::currency::delete(&pool, GUILD_A, &format!("delete {UNIT_A}"))
            .await
            .unwrap(),
        vc_core::currency::DeleteResult::Deleted
    );
    vc_core::currency::create(&pool, GUILD_A, "replacement", UNIT_A, PERSON, 1000)
        .await
        .unwrap();
    let allowed = request(
        router(pool),
        "POST",
        "/api/v2/users/@me/transactions",
        Some(&token),
        pay(UNIT_A, "100"),
    )
    .await;
    assert_eq!(allowed.status, 201);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn read_only_grant_cannot_approve_a_contract(pool: PgPool) {
    fixture(&pool).await;
    let (application, token, _) = ask_and_approve(
        &pool,
        Some(json!([format!("{SITE}/api/v2/currencies/{CURRENCY_A}")])),
        &["vc.read"],
    )
    .await;
    let contract = vc_core::contract::create(
        &pool,
        application,
        UNIT_B,
        &[vc_core::contract::NewParty {
            discord_id: PERSON,
            amount: 100,
        }],
        Some(RECEIVER),
        None,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    let approved = request(
        router(pool.clone()),
        "POST",
        &format!("/api/v2/contracts/{contract}/approval"),
        Some(&token),
        Value::Null,
    )
    .await;
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2")
            .bind(i64::from(PERSON_ACCOUNT))
            .bind(CURRENCY_B)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        approved.status == 401 || approved.status == 403,
        "read-only grant for A approved a contract in B: status {}, B wallet {balance} (was 1000)",
        approved.status
    );
    assert_eq!(balance, 1000);
    let refused = request(
        router(pool.clone()),
        "POST",
        &format!("/api/v2/contracts/{contract}/refusal"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(refused.status, 403);
    let own = mint(&pool, PERSON_ACCOUNT, &[]).await;
    let approved = request(
        router(pool.clone()),
        "POST",
        &format!("/api/v2/contracts/{contract}/approval"),
        Some(&own),
        Value::Null,
    )
    .await;
    assert_eq!(approved.status, 200);
    let withdrawn = request(
        router(pool.clone()),
        "DELETE",
        &format!("/api/v2/contracts/{contract}/approval"),
        Some(&token),
        Value::Null,
    )
    .await;
    assert_eq!(withdrawn.status, 403);
    let still_active = vc_core::contract::find(&pool, contract)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_active.status, "active");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn currency_restriction_applies_to_claim_approval(pool: PgPool) {
    fixture(&pool).await;
    let (_, token, _) = ask_and_approve(
        &pool,
        Some(json!([format!("{SITE}/api/v2/currencies/{CURRENCY_A}")])),
        &["vc.claim"],
    )
    .await;
    insert_claim(
        &pool,
        1,
        100,
        "pending",
        RECEIVER_ACCOUNT,
        PERSON_ACCOUNT,
        CURRENCY_B,
    )
    .await;
    let approved = request(
        router(pool.clone()),
        "PATCH",
        "/api/v2/users/@me/claims/1",
        Some(&token),
        json!({"status":"approved"}),
    )
    .await;
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id = $1 AND currency_id = $2")
            .bind(i64::from(PERSON_ACCOUNT))
            .bind(CURRENCY_B)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        approved.status, 403,
        "grant restricted to A approved a claim in B: B wallet {balance} (was 1000)"
    );
    assert_eq!(balance, 1000);
    let read = get(
        router(pool.clone()),
        "/api/v2/users/@me/claims/1",
        Some(&token),
    )
    .await;
    assert_eq!(read.status, 403);
    for body in [
        json!({"status":"denied"}),
        json!({"metadata":{"key":"value"}}),
    ] {
        assert_eq!(
            request(
                router(pool.clone()),
                "PATCH",
                "/api/v2/users/@me/claims/1",
                Some(&token),
                body
            )
            .await
            .status,
            403
        );
    }
    let created = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/claims",
        Some(&token),
        json!({"unit":UNIT_B,"amount":"100","payer_discord_id":RECEIVER.to_string()}),
    )
    .await;
    assert_eq!(created.status, 403);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let claim_status: String = sqlx::query_scalar("SELECT status::text FROM claims WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(claim_status, "pending");
    insert_claim(
        &pool,
        2,
        100,
        "pending",
        RECEIVER_ACCOUNT,
        PERSON_ACCOUNT,
        CURRENCY_A,
    )
    .await;
    let allowed = request(
        router(pool.clone()),
        "PATCH",
        "/api/v2/users/@me/claims/2",
        Some(&token),
        json!({"status":"approved"}),
    )
    .await;
    assert_eq!(allowed.status, 200);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn delegated_read_scope_is_independent_of_write_scopes(pool: PgPool) {
    fixture(&pool).await;
    let app = insert_application(&pool, OWNER, "contract owner").await;
    let id = vc_core::contract::create(
        &pool,
        app,
        UNIT_A,
        &[vc_core::contract::NewParty {
            discord_id: PERSON,
            amount: 100,
        }],
        Some(RECEIVER),
        None,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .unwrap();
    insert_claim(
        &pool,
        1,
        100,
        "pending",
        RECEIVER_ACCOUNT,
        PERSON_ACCOUNT,
        CURRENCY_A,
    )
    .await;
    for (scope, expected) in [("vc.read", 200), ("vc.pay", 403), ("vc.claim", 403)] {
        let (_, token, _) = ask_and_approve(&pool, None, &[scope]).await;
        for path in [
            "/api/v2/users/@me".to_owned(),
            "/api/v2/users/@me/balances".to_owned(),
            "/api/v2/users/@me/claims".to_owned(),
            "/api/v2/users/@me/claims/1".to_owned(),
            "/api/v2/users/@me/contracts".to_owned(),
            format!("/api/v2/contracts/{id}"),
            format!("/api/v2/contracts/{id}/payments"),
        ] {
            let r = get(router(pool.clone()), &path, Some(&token)).await;
            assert_eq!(r.status, expected, "{scope} GET {path}");
        }
        if scope == "vc.read" {
            for (method, path, body) in [
                (
                    "POST",
                    "/api/v2/users/@me/transactions",
                    json!({"unit":UNIT_A,"amount":"10","receiver_discord_id":RECEIVER.to_string()}),
                ),
                (
                    "POST",
                    "/api/v2/users/@me/claims",
                    json!({"unit":UNIT_A,"amount":"10","payer_discord_id":RECEIVER.to_string()}),
                ),
                (
                    "PATCH",
                    "/api/v2/users/@me/claims/1",
                    json!({"status":"approved"}),
                ),
            ] {
                assert_eq!(
                    request(router(pool.clone()), method, path, Some(&token), body)
                        .await
                        .status,
                    403,
                    "{method} {path}"
                );
            }
        }
        for (method, path, body) in [
            (
                "POST",
                format!("/api/v2/contracts/{id}/approval"),
                Value::Null,
            ),
            (
                "POST",
                format!("/api/v2/contracts/{id}/refusal"),
                Value::Null,
            ),
            (
                "DELETE",
                format!("/api/v2/contracts/{id}/approval"),
                Value::Null,
            ),
            (
                "POST",
                "/api/v2/contracts".to_owned(),
                json!({"unit":UNIT_A,"parties":[{"discord_id":PERSON.to_string(),"amount":"100"}]}),
            ),
            (
                "POST",
                format!("/api/v2/contracts/{id}/payments"),
                json!({"amount":"10","receiver_discord_id":RECEIVER.to_string()}),
            ),
        ] {
            assert_eq!(
                request(router(pool.clone()), method, &path, Some(&token), body)
                    .await
                    .status,
                403,
                "{scope} {method} {path}"
            );
        }
    }
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id=$1 AND currency_id=$2")
            .bind(i64::from(PERSON_ACCOUNT))
            .bind(CURRENCY_A)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 1000);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn contract_currency_filter_precedes_pagination_and_covers_ledger(pool: PgPool) {
    fixture(&pool).await;
    let (app, token, _) = ask_and_approve(
        &pool,
        Some(json!([format!("{SITE}/api/v2/currencies/{CURRENCY_A}")])),
        &["vc.read"],
    )
    .await;
    let mut ids = Vec::new();
    for unit in [UNIT_A, UNIT_B, UNIT_A, UNIT_B, UNIT_B] {
        ids.push(
            vc_core::contract::create(
                &pool,
                app,
                unit,
                &[vc_core::contract::NewParty {
                    discord_id: PERSON,
                    amount: 100,
                }],
                Some(RECEIVER),
                None,
                time::OffsetDateTime::now_utc(),
            )
            .await
            .unwrap(),
        );
    }
    for (index, expected) in [(0, 200), (1, 403), (2, 200), (3, 403), (4, 403)] {
        for suffix in ["", "/payments"] {
            let r = get(
                router(pool.clone()),
                &format!("/api/v2/contracts/{}{suffix}", ids[index]),
                Some(&token),
            )
            .await;
            assert_eq!(r.status, expected);
        }
    }
    let first = get(
        router(pool.clone()),
        "/api/v2/users/@me/contracts?limit=1",
        Some(&token),
    )
    .await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body.as_array().unwrap().len(), 1);
    assert_eq!(first.body[0]["id"], ids[2].to_string());
    assert!(
        first.headers["link"]
            .to_str()
            .unwrap()
            .contains(&format!("next={}", ids[2]))
    );
    let second = get(
        router(pool.clone()),
        &format!("/api/v2/users/@me/contracts?limit=1&next={}", ids[2]),
        Some(&token),
    )
    .await;
    assert_eq!(second.body[0]["id"], ids[0].to_string());
    let end = get(
        router(pool.clone()),
        &format!("/api/v2/users/@me/contracts?limit=1&next={}", ids[0]),
        Some(&token),
    )
    .await;
    assert_eq!(end.body, json!([]));
    let inclusive = get(
        router(pool.clone()),
        &format!("/api/v2/users/@me/contracts?limit=2&on_next={}", ids[2]),
        Some(&token),
    )
    .await;
    assert_eq!(inclusive.body.as_array().unwrap().len(), 2);
    let own = mint(&pool, PERSON_ACCOUNT, &[]).await;
    let all = get(
        router(pool.clone()),
        "/api/v2/users/@me/contracts",
        Some(&own),
    )
    .await;
    assert_eq!(all.body.as_array().unwrap().len(), 5);
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn legacy_application_scopes_still_read_write_claims_and_pay(pool: PgPool) {
    fixture(&pool).await;
    let app = insert_application(&pool, OWNER, "legacy app").await;
    let account = account_of(&pool, app).await;
    insert_asset(&pool, account, CURRENCY_A, 1000).await;
    // These are the old application's scopes. It has never requested vc.read.
    let token = mint_app(&pool, account, &["vc.claim", "vc.pay"]).await;
    for path in [
        "/api/v2/users/@me",
        "/api/v2/users/@me/balances",
        "/api/v2/users/@me/claims",
    ] {
        assert_eq!(
            get(router(pool.clone()), path, Some(&token)).await.status,
            200,
            "{path}"
        );
    }
    let created = request(
        router(pool.clone()),
        "POST",
        "/api/v2/users/@me/claims",
        Some(&token),
        json!({"unit":UNIT_A,"amount":"10","payer_discord_id":PERSON.to_string()}),
    )
    .await;
    assert_eq!(created.status, 201);
    let claim_id: i64 = sqlx::query_scalar("SELECT id FROM claims WHERE claimant_user_id=$1")
        .bind(i64::from(account))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        get(
            router(pool.clone()),
            &format!("/api/v2/users/@me/claims/{claim_id}"),
            Some(&token)
        )
        .await
        .status,
        200
    );
    // The old API can also address an application's account as a payer.
    insert_claim(
        &pool,
        100,
        10,
        "pending",
        PERSON_ACCOUNT,
        account,
        CURRENCY_A,
    )
    .await;
    assert_eq!(
        request(
            router(pool.clone()),
            "PATCH",
            "/api/v2/users/@me/claims/100",
            Some(&token),
            json!({"status":"approved"})
        )
        .await
        .status,
        200
    );
    assert_eq!(
        request(
            router(pool.clone()),
            "POST",
            "/api/v2/users/@me/transactions",
            Some(&token),
            json!({"unit":UNIT_A,"amount":"10","receiver_discord_id":RECEIVER.to_string()})
        )
        .await
        .status,
        201
    );
    let balance: i64 =
        sqlx::query_scalar("SELECT amount FROM assets WHERE user_id=$1 AND currency_id=$2")
            .bind(i64::from(account))
            .bind(CURRENCY_A)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(balance, 980);
    // Lacking vc.claim must still deny an old application's claim reads.
    let pay_only = mint_app(&pool, account, &["vc.pay"]).await;
    assert_eq!(
        get(
            router(pool.clone()),
            "/api/v2/users/@me/claims",
            Some(&pay_only)
        )
        .await
        .status,
        403
    );
}
