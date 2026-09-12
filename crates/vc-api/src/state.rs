use std::sync::Arc;

use sqlx::PgPool;
use vc_auth::AuthState;

use crate::discord::DiscordApi;

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    jwt_secret: Arc<Vec<u8>>,
    discord: Arc<dyn DiscordApi>,
}

impl AppState {
    pub fn new(pool: PgPool, jwt_secret: impl Into<Vec<u8>>, discord: Arc<dyn DiscordApi>) -> Self {
        Self {
            pool,
            jwt_secret: Arc::new(jwt_secret.into()),
            discord,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn discord(&self) -> &Arc<dyn DiscordApi> {
        &self.discord
    }
}

impl AuthState for AppState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn jwt_secret(&self) -> &[u8] {
        &self.jwt_secret
    }
}
