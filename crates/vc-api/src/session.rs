//! The browser's session, as a signed cookie.
//!
//! Signed and not encrypted, like the app this replaces: `Plug.Session` was
//! configured with a `signing_salt` and a comment saying its contents can be
//! read but not tampered with. So this is the same kind of cookie — readable,
//! tamper-proof — and not something more elaborate.

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// The cookie the session travels in, and how it is marked. `HttpOnly` because
/// only the server reads it: an SPA uses the API, not the cookie. `SameSite=Lax`
/// and `Secure` in production are the old app's settings.
pub const COOKIE_NAME: &str = "_virtualcrypto_session";

/// A login waiting for Discord to come back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginAttempt {
    /// Compared against the `state` Discord returns.
    pub state: String,
    /// Where to go afterwards. `continue` is a Rust keyword, hence the name.
    pub continue_to: String,
}

/// What the browser carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Session {
    /// Not decoration. The session cookie and the API's bearer tokens are both
    /// JWTs, and different secrets are only half a defence if one can be
    /// replayed as the other.
    typ: Type,
    /// Who is logged in, once someone is.
    #[serde(default)]
    pub user_id: Option<i64>,
    /// A login in flight, before Discord has answered.
    #[serde(default)]
    pub discord_oauth2: Option<LoginAttempt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Type {
    #[serde(rename = "session")]
    #[default]
    Session,
}

impl Session {
    /// A session with nobody logged in.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sign(&self, secret: &[u8]) -> Result<String, jsonwebtoken::errors::Error> {
        encode(
            &Header::new(Algorithm::HS256),
            self,
            &EncodingKey::from_secret(secret),
        )
    }

    /// A session, or nothing.
    ///
    /// A tampered, foreign or unparseable cookie answers the same as no cookie
    /// at all — which is what the old app does, and why the caller never has to
    /// distinguish "logged out" from "logged out badly".
    pub fn parse(value: &str, secret: &[u8]) -> Option<Self> {
        let mut validation = Validation::new(Algorithm::HS256);
        // The old session expires by its Discord authorization going stale, not
        // by a claim of its own, so there is nothing here for jsonwebtoken to
        // require or validate.
        validation.required_spec_claims.clear();

        decode::<Self>(value, &DecodingKey::from_secret(secret), &validation)
            .ok()
            .map(|data| data.claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"a session secret";

    #[test]
    fn a_session_survives_the_round_trip() {
        let session = Session {
            user_id: Some(42),
            discord_oauth2: Some(LoginAttempt {
                state: "the-state".to_owned(),
                continue_to: "/me".to_owned(),
            }),
            ..Session::default()
        };

        let cookie = session.sign(SECRET).expect("a signed cookie");

        assert_eq!(Session::parse(&cookie, SECRET), Some(session));
    }

    #[test]
    fn a_tampered_cookie_is_not_a_session() {
        let cookie = Session {
            user_id: Some(42),
            ..Session::default()
        }
        .sign(SECRET)
        .expect("a signed cookie");

        // Change a character of the payload, keeping the token's shape. The
        // fifth is chosen because a base64 character is six bits wide, so this
        // one is certain to land in the decoded bytes rather than in padding.
        let mut parts: Vec<String> = cookie.split('.').map(str::to_owned).collect();
        let mut payload: Vec<char> = parts[1].chars().collect();
        payload[4] = if payload[4] == 'A' { 'B' } else { 'A' };
        parts[1] = payload.into_iter().collect();
        let tampered = parts.join(".");

        assert_eq!(Session::parse(&tampered, SECRET), None);
    }

    #[test]
    fn another_secret_is_not_a_session() {
        let cookie = Session {
            user_id: Some(42),
            ..Session::default()
        }
        .sign(SECRET)
        .expect("a signed cookie");

        assert_eq!(Session::parse(&cookie, b"another secret"), None);
    }

    /// The reason for the `typ` claim: an API token must not be usable here.
    #[test]
    fn a_token_that_is_not_a_session_is_refused() {
        #[derive(Serialize)]
        struct Foreign<'a> {
            typ: &'a str,
            sub: &'a str,
        }

        let foreign = encode(
            &Header::new(Algorithm::HS256),
            &Foreign {
                typ: "access",
                sub: "42",
            },
            &EncodingKey::from_secret(SECRET),
        )
        .expect("a signed token");

        assert_eq!(Session::parse(&foreign, SECRET), None);
    }

    #[test]
    fn rubbish_is_not_a_session() {
        assert_eq!(Session::parse("", SECRET), None);
        assert_eq!(Session::parse("not.a.jwt", SECRET), None);
    }

    #[test]
    fn a_new_session_has_nobody_in_it() {
        let session = Session::new();

        assert_eq!(session.user_id, None);
        assert_eq!(session.discord_oauth2, None);
    }
}
