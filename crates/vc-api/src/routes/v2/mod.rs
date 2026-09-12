pub mod users;

use axum::Router;
use axum::routing::get;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v2/users/@me", get(users::me))
}
