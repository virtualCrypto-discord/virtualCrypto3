use sqlx::PgPool;

/// What the authentication extractor needs from the application state.
///
/// Declared here so `vc-auth` does not depend on the API crate that owns the
/// concrete state type.
pub trait AuthState: Send + Sync {
    fn pool(&self) -> &PgPool;
    fn jwt_secret(&self) -> &[u8];
}
