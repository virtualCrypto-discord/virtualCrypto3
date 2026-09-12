//! Guardian-compatible JWT authentication and authorization scopes.
//!
//! Tokens are HS512 JWTs issued by the Elixir service, so verification has to
//! accept exactly what Guardian produces: `iss`/`aud` of `virtualCrypto`, a
//! string `sub`, and the custom `scopes`, `kind`, `jti` and `typ` claims.

pub mod claims;
pub mod error;
pub mod extractor;
pub mod issue;
pub mod jwt;
pub mod state;

pub use claims::{AUDIENCE, Claims, ISSUER, Kind, Scopes};
pub use error::AuthError;
pub use extractor::AuthUser;
pub use state::AuthState;
