//! Contract tests for `GET /api/v2/users/@me/claims`, ported from the Elixir
//! suite (`test/virtualCrypto_web/controllers/api/v2/claim/claim_controller_test.exs`).
//!
//! The Elixir fixture (`setup_claim/1`) builds six claims covering every status,
//! plus a claim where the claimant and payer are the same user. The same state is
//! inserted here.

mod support;

use serde_json::{Value, json};
use sqlx::PgPool;
use support::{
    CLAIM_AT_RFC3339, Response, assert_json, fake, get, insert_asset, insert_claim,
    insert_currency, insert_user, mint, state,
};

const GUILD: i64 = 900_000_000_000_000_001;
const CURRENCY_ID: i64 = 1;
const USER1: i32 = 1;
const USER2: i32 = 2;
const USER3: i32 = 3;
const DISCORD1: i64 = 100_000_000_000_000_001;
const DISCORD2: i64 = 100_000_000_000_000_002;
const DISCORD3: i64 = 100_000_000_000_000_003;

fn claim(id: i64, amount: i64, claimant: i32, payer: i32, status: &str) -> Value {
    let discord = |user: i32| match user {
        USER1 => DISCORD1,
        _ => DISCORD2,
    };

    json!({
        "id": id.to_string(),
        "currency": {
            "name": "nyan",
            "unit": "nyan",
            "guild": GUILD.to_string(),
            "pool_amount": "500",
        },
        "amount": amount.to_string(),
        "claimant": {
            "id": claimant.to_string(),
            "discord": { "id": discord(claimant).to_string() },
        },
        "payer": {
            "id": payer.to_string(),
            "discord": { "id": discord(payer).to_string() },
        },
        "created_at": CLAIM_AT_RFC3339,
        "updated_at": CLAIM_AT_RFC3339,
        "status": status,
        "metadata": {},
    })
}

/// claim 1: user1 -> user2, 500, pending
/// claim 2: user2 -> user1, 9999999, pending
/// claim 3: user1 -> user2, 500, approved
/// claim 4: user1 -> user2, 500, denied
/// claim 5: user1 -> user2, 500, canceled
/// claim 6: user1 -> user1, 100, pending
fn expected(entries: &[(i64, i64, i32, i32, &str)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(id, amount, claimant, payer, status)| {
                claim(*id, *amount, *claimant, *payer, status)
            })
            .collect(),
    )
}

async fn fixture(pool: &PgPool) {
    insert_user(pool, USER1, DISCORD1).await;
    insert_user(pool, USER2, DISCORD2).await;
    insert_user(pool, USER3, DISCORD3).await;
    insert_currency(pool, CURRENCY_ID, "nyan", "nyan", GUILD, 500).await;
    insert_asset(pool, USER1, CURRENCY_ID, 199_500).await;
    insert_asset(pool, USER2, CURRENCY_ID, 1_000).await;

    insert_claim(pool, 1, 500, "pending", USER1, USER2, CURRENCY_ID).await;
    insert_claim(pool, 2, 9_999_999, "pending", USER2, USER1, CURRENCY_ID).await;
    insert_claim(pool, 3, 500, "approved", USER1, USER2, CURRENCY_ID).await;
    insert_claim(pool, 4, 500, "denied", USER1, USER2, CURRENCY_ID).await;
    insert_claim(pool, 5, 500, "canceled", USER1, USER2, CURRENCY_ID).await;
    insert_claim(pool, 6, 100, "pending", USER1, USER1, CURRENCY_ID).await;
}

async fn list(pool: PgPool, query: &str, token: &str) -> Response {
    let uri = if query.is_empty() {
        "/api/v2/users/@me/claims".to_string()
    } else {
        format!("/api/v2/users/@me/claims?{query}")
    };

    get(vc_api::router(state(pool, fake())), &uri, Some(token)).await
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn user_one_sees_their_pending_claims_newest_first(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn user_two_sees_their_pending_claims(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER2, &["vc.claim"]).await;

    let response = list(pool, "", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn statuses_select_several_states(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "statuses[]=pending&statuses[]=denied", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (4, 500, USER1, USER2, "denied"),
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_status_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "statuses[]=nyan", &token).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_statuses" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn received_only_returns_claims_the_user_pays(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "type=received", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (2, 9_999_999, USER2, USER1, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn claimed_only_returns_claims_the_user_raised(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "type=claimed", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unknown_type_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "type=nyan", &token).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_type" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_non_numeric_limit_is_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "limit=nyan", &token).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_limit" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn on_next_and_next_together_are_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "on_next=6&next=6", &token).await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_cursor" }),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn both_related_user_forms_together_are_rejected(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(
        pool,
        "related_discord_user_id=1&related_vc_user_id=1",
        &token,
    )
    .await;

    assert_json(
        &response,
        400,
        json!({ "error": "invalid_request", "error_description": "invalid_related_user" }),
    );
}

/// The related-user filter is applied *in addition* to the side filter, so this
/// selects the pending claims between the two users, in either direction.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn related_vc_user_narrows_to_claims_between_the_two_users(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "related_vc_user_id=2", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn ascending_order_reverses_the_result(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "order=asc_claim_id", &token).await;

    assert_json(
        &response,
        200,
        expected(&[
            (1, 500, USER1, USER2, "pending"),
            (2, 9_999_999, USER2, USER1, "pending"),
            (6, 100, USER1, USER1, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn next_is_exclusive_and_on_next_is_inclusive(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let exclusive = list(pool.clone(), "next=6", &token).await;
    assert_json(
        &exclusive,
        200,
        expected(&[
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );

    let inclusive = list(pool, "on_next=6", &token).await;
    assert_json(
        &inclusive,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (2, 9_999_999, USER2, USER1, "pending"),
            (1, 500, USER1, USER2, "pending"),
        ]),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_full_page_carries_a_link_header(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool.clone(), "limit=2", &token).await;
    assert_json(
        &response,
        200,
        expected(&[
            (6, 100, USER1, USER1, "pending"),
            (2, 9_999_999, USER2, USER1, "pending"),
        ]),
    );
    assert!(
        response.headers.contains_key("link"),
        "a full page advertises the next one"
    );

    let partial = list(pool, "limit=5", &token).await;
    assert!(
        !partial.headers.contains_key("link"),
        "a partial page has no next page"
    );
}

/// A list that answers every matching row when the caller says nothing spends a
/// caller's memory in proportion to their data, on a request that did not ask for
/// it. Fifty is the page and `link` says where the next one is — the scan the
/// contract lists already make, and the one this list was the last to make.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_absent_limit_is_a_page(pool: PgPool) {
    fixture(&pool).await;

    // Forty-eight more pending claims between the two users, so the list holds one
    // row more than a page.
    for id in 7..=54 {
        insert_claim(&pool, id, 100, "pending", USER1, USER2, CURRENCY_ID).await;
    }

    let token = mint(&pool, USER1, &["vc.claim"]).await;
    let response = list(pool.clone(), "", &token).await;

    assert_eq!(response.status, 200, "body: {}", response.body);

    let rows = response.body.as_array().expect("an array");
    assert_eq!(rows.len(), 50, "a page, not everything: {}", response.body);
    assert_eq!(rows[0]["id"], "54", "newest first");
    assert_eq!(rows[49]["id"], "2", "and the page ends at the fiftieth");

    let link = response
        .headers
        .get("link")
        .expect("a full page advertises the next one")
        .to_str()
        .expect("a header");
    assert!(link.contains("next=2"), "{link}");

    // The row the page left over is the one the link leads to.
    let rest = list(pool, "next=2", &token).await;
    assert_eq!(rest.status, 200, "body: {}", rest.body);
    assert_eq!(
        rest.body,
        expected(&[(1, 500, USER1, USER2, "pending")]),
        "the rest of the list"
    );
    assert!(
        !rest.headers.contains_key("link"),
        "and a partial page has no next page"
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_token_without_the_claim_scope_is_forbidden(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.pay"]).await;

    let response = list(pool, "", &token).await;

    assert_json(
        &response,
        403,
        json!({ "error": "invalid_token", "error_description": "permission_denied" }),
    );
}

/// A negative limit is the caller's typo rather than a question to put to
/// Postgres: this is not one of the crashes `docs/known-gaps.md` reproduces (the
/// cursor's and the order parameter's are), and the non-numeric limit beside it
/// is already a 400.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_negative_limit_is_invalid(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "limit=-1", &token).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error_description"], "invalid_limit");
}

/// A cursor and an order that cannot be read are the caller's mistakes, and each
/// is answered with the 400 that names it — the same `invalid_cursor` two cursors
/// at once already got. The 500s the Elixir produces here are not reproduced;
/// `docs/known-gaps.md` says why.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_cursor_or_an_order_that_cannot_be_read_is_400(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    for (query, complaint) in [
        ("next=abc", "invalid_cursor"),
        ("on_next=abc", "invalid_cursor"),
        ("order=nonsense", "invalid_order"),
    ] {
        let response = list(pool.clone(), query, &token).await;

        assert_eq!(response.status, 400, "{query}: {}", response.body);
        assert_eq!(response.body["error_description"], complaint, "{query}");
    }
}

/// The ceiling is the reader's, so the claim list has it too, and the refusal is
/// the one it already answers a bad limit with.
#[sqlx::test(migrations = "../vc-core/migrations")]
async fn a_limit_above_the_ceiling_is_invalid(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;

    let response = list(pool, "limit=1000000", &token).await;

    assert_eq!(response.status, 400, "body: {}", response.body);
    assert_eq!(response.body["error_description"], "invalid_limit");
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn an_unregistered_related_discord_user_matches_no_claims(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;
    let response = list(pool, "related_discord_user_id=777777777777777777", &token).await;
    assert_json(&response, 200, json!([]));
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn following_links_preserves_related_users(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;
    for (key, id) in [
        ("related_vc_user_id", i64::from(USER2)),
        ("related_discord_user_id", DISCORD2),
    ] {
        let first = list(pool.clone(), &format!("{key}={id}&limit=1"), &token).await;
        assert_eq!(first.status, 200, "{}", first.body);
        assert_eq!(first.body[0]["id"], "2");
        let link = first.headers.get("link").unwrap().to_str().unwrap();
        let query = link.split_once('?').unwrap().1.split_once('>').unwrap().0;
        assert!(query.contains(&format!("{key}={id}")), "{link}");
        let next = list(pool.clone(), query, &token).await;
        assert_eq!(next.status, 200, "link: {link}; body: {}", next.body);
        assert_eq!(next.body.as_array().unwrap().len(), 1);
        assert_eq!(next.body[0]["id"], "1");
    }
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn related_user_ids_require_unsigned_positive_decimal_digits(pool: PgPool) {
    fixture(&pool).await;
    let token = mint(&pool, USER1, &["vc.claim"]).await;
    for key in ["related_vc_user_id", "related_discord_user_id"] {
        for value in [
            "%2B2",
            "-2",
            "0",
            "",
            "%202",
            "2%20",
            "2.0",
            "9223372036854775808",
        ] {
            let response = list(pool.clone(), &format!("{key}={value}"), &token).await;
            assert_eq!(response.status, 400, "{key}={value}: {}", response.body);
            assert_eq!(response.body["error_description"], "invalid_related_user");
        }
    }
}
