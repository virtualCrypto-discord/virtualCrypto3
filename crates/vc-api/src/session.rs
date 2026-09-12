//! The browser's session, as a signed cookie.
//!
//! Signed and not encrypted, like the app this replaces: `Plug.Session` was
//! configured with a `signing_salt` and a comment saying its contents can be
//! read but not tampered with. So this is the same kind of cookie — readable,
//! tamper-proof — and not something more elaborate.

use axum::http::HeaderMap;
use axum::http::header::HeaderValue;
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

/// The session a request carries, if it carries one worth trusting.
pub fn from_headers(headers: &HeaderMap, secret: &[u8]) -> Option<Session> {
    let cookie = headers.get(axum::http::header::COOKIE)?;
    Session::parse(cookie_value(cookie, COOKIE_NAME)?, secret)
}

/// The value of one cookie in a `Cookie` header.
///
/// Splitting this by hand is safe here in a way it would not be in general: the
/// value is a JWT, so it is base64-url and dots, and cannot contain the
/// separators, a space or a quote.
pub fn cookie_value<'a>(header: &'a HeaderValue, name: &str) -> Option<&'a str> {
    let header = header.to_str().ok()?;

    header.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim())
    })
}

/// A `Set-Cookie` that plants the session.
///
/// `HttpOnly` because only the server reads it — the SPA uses the API — and
/// `SameSite=Lax` and `Secure` as the old app's `Plug.Session` had them.
pub fn set_cookie(
    session: &Session,
    secret: &[u8],
    secure: bool,
) -> Result<HeaderValue, jsonwebtoken::errors::Error> {
    let cookie = format!(
        "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Lax{}",
        session.sign(secret)?,
        if secure { "; Secure" } else { "" }
    );

    Ok(HeaderValue::from_str(&cookie).expect("a cookie is a valid header value"))
}

/// A `Set-Cookie` that ends the session, which is how logout works.
pub fn clear_cookie(secure: bool) -> HeaderValue {
    let cookie = format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    );

    HeaderValue::from_str(&cookie).expect("a cookie is a valid header value")
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

#[cfg(test)]
mod cookie_tests {
    use super::*;

    const SECRET: &[u8] = b"a session secret";

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn a_request_can_carry_a_session() {
        let session = Session {
            user_id: Some(7),
            ..Session::default()
        };

        let carried = set_cookie(&session, SECRET, false).expect("a cookie");
        let headers = headers_with_cookie(carried.to_str().unwrap().split(';').next().unwrap());

        assert_eq!(from_headers(&headers, SECRET), Some(session));
    }

    #[test]
    fn the_session_is_found_among_other_cookies() {
        let session = Session {
            user_id: Some(7),
            ..Session::default()
        };
        let value = session.sign(SECRET).expect("a signed cookie");

        let headers = headers_with_cookie(&format!("other=1; {COOKIE_NAME}={value}; another=2"));

        assert_eq!(from_headers(&headers, SECRET), Some(session));
    }

    #[test]
    fn a_request_without_one_carries_nothing() {
        assert_eq!(from_headers(&HeaderMap::new(), SECRET), None);
        assert_eq!(from_headers(&headers_with_cookie("other=1"), SECRET), None);
    }

    #[test]
    fn the_cookie_is_marked_http_only_and_lax() {
        let cookie = set_cookie(&Session::new(), SECRET, false)
            .expect("a cookie")
            .to_str()
            .unwrap()
            .to_owned();

        assert!(cookie.starts_with(&format!("{COOKIE_NAME}=")), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(cookie.contains("Path=/"), "{cookie}");
        assert!(!cookie.contains("Secure"), "{cookie}");
    }

    #[test]
    fn the_cookie_is_secure_when_it_is_asked_to_be() {
        let cookie = set_cookie(&Session::new(), SECRET, true)
            .expect("a cookie")
            .to_str()
            .unwrap()
            .to_owned();

        assert!(cookie.contains("; Secure"), "{cookie}");
    }

    #[test]
    fn clearing_the_cookie_ends_it() {
        let cookie = clear_cookie(false).to_str().unwrap().to_owned();

        assert!(cookie.starts_with(&format!("{COOKIE_NAME}=;")), "{cookie}");
        assert!(cookie.contains("Max-Age=0"), "{cookie}");
    }
}
