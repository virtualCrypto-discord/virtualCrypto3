pub mod claims;
pub mod currencies;
pub mod transactions;
pub mod users;

use axum::Router;
use axum::routing::get;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v2/users/@me", get(users::me))
        .route("/api/v2/users/@me/balances", get(users::balances))
        .route(
            "/api/v2/users/@me/claims",
            get(claims::index).post(claims::create),
        )
        .route(
            "/api/v2/users/@me/claims/{id}",
            get(claims::get_by_id).patch(claims::patch),
        )
        .route(
            "/api/v2/users/@me/transactions",
            axum::routing::post(transactions::post),
        )
        .route("/api/v2/currencies", get(currencies::index))
        .route("/api/v2/currencies/{id}", get(currencies::show))
}
