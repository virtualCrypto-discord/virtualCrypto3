//! `GET /api/v2/users/@me/balances`, from the goldens captured for it.
//!
//! The two here are the endpoint's own refusals, and they need no holdings: a
//! request with no `Authorization` and one with a token that is not a token are
//! answered before anything is looked up. The other two goldens need a currency and
//! an asset each, which is a fixture rather than a value.

mod support;

use sqlx::PgPool;
use support::{assert_matches_golden, fake, get, golden, state};

const URI: &str = "/api/v2/users/@me/balances";

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn without_authorization_is_400(pool: PgPool) {
    let response = get(vc_api::router(state(pool, fake())), URI, None).await;

    assert_matches_golden(
        &response,
        &golden(include_str!("golden/v2_users_me_balances_no_auth.json")),
    );
}

#[sqlx::test(migrations = "../vc-core/migrations")]
async fn with_a_malformed_token_is_401(pool: PgPool) {
    let response = get(vc_api::router(state(pool, fake())), URI, Some("not-a-jwt")).await;

    assert_matches_golden(
        &response,
        &golden(include_str!(
            "golden/v2_users_me_balances_garbage_token.json"
        )),
    );
}
