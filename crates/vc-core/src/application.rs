//! Applications, and the rules an OAuth2 client has to satisfy.
//!
//! The redirect-URI rule below is the one `Auth.RedirectUris` sounds like it
//! should own, but does not: in the Elixir it is written out at two call sites —
//! `auth/internal/application.ex` for registration and
//! `application_patch_query_service.ex` for an edit — and the module is only an
//! Ecto schema. So there is nothing to port from there, and this is the rule.

/// Why a set of redirect URIs was refused, in the API's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectUriError {
    /// `redirect_uri_scheme_must_be_http_or_https`. The Elixir's answer to every
    /// URI that is not plainly `http` or `https`, including a relative one.
    Scheme,
}

/// The one scope the service has ever accepted.
pub const OPENID: &str = "openid";

/// Why a list of scopes was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeError {
    /// `invalid_scope`. The Elixir answers the same way for a repeat and for an
    /// unknown name, so there is one reason here as well.
    Invalid,
}

/// `is_valid_scopes?/1`: no repeats, and nothing but `openid`.
///
/// Which leaves exactly two acceptable answers, `[]` and `["openid"]` — so the
/// consent screen's `scope` is usually nothing at all. Written as the two rules
/// the Elixir states rather than as `len() <= 1`, because "no repeats" is the
/// half a caller would not think to check.
pub fn check_scopes(scopes: &[String]) -> Result<(), ScopeError> {
    let unique: std::collections::HashSet<&String> = scopes.iter().collect();

    if unique.len() != scopes.len() || scopes.iter().any(|scope| scope != OPENID) {
        Err(ScopeError::Invalid)
    } else {
        Ok(())
    }
}

/// Every registered redirect URI must carry an `http` or `https` scheme.
///
/// An empty list passes, as `Enum.all?/2` on an empty list does; whether an
/// application may register none at all is a separate question, asked where the
/// payload is read.
pub fn check_redirect_uris(uris: &[String]) -> Result<(), RedirectUriError> {
    if uris
        .iter()
        .all(|uri| matches!(scheme(uri).as_deref(), Some("http" | "https")))
    {
        Ok(())
    } else {
        Err(RedirectUriError::Scheme)
    }
}

/// The scheme a URI declares, if it declares one.
///
/// This is `URI.parse/1`'s notion of a scheme and deliberately no more: the
/// leading RFC 3986 `label:` before any `/`, `?` or `#`, lowercased. It is not a
/// URL parser. `http:example` therefore has a scheme of `http`, which is what the
/// Elixir accepts — a stricter parser here would refuse values it took, and the
/// point of the rule is to keep browsers from being sent to `javascript:`.
fn scheme(uri: &str) -> Option<String> {
    let end = uri.find([':', '/', '?', '#'])?;
    if !uri.as_bytes()[end].eq(&b':') {
        return None;
    }

    let scheme = &uri[..end];

    let mut chars = scheme.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }

    Some(scheme.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(uris: &[&str]) -> Result<(), RedirectUriError> {
        check_redirect_uris(
            &uris
                .iter()
                .map(|uri| (*uri).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn http_and_https_are_what_the_rule_is_for() {
        assert_eq!(check(&["https://example.com/callback"]), Ok(()));
        assert_eq!(check(&["http://localhost:8080/callback"]), Ok(()));
        assert_eq!(
            check(&["https://a.example/cb", "http://b.example/cb"]),
            Ok(())
        );
    }

    /// The case the rule exists for: a browser is about to be sent here.
    #[test]
    fn a_scheme_a_browser_would_execute_is_refused() {
        assert_eq!(
            check(&["javascript:alert(1)"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&["data:text/html,x"]), Err(RedirectUriError::Scheme));
        assert_eq!(
            check(&["file:///etc/passwd"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&["ftp://example.com"]), Err(RedirectUriError::Scheme));
    }

    #[test]
    fn a_uri_with_no_scheme_is_refused() {
        assert_eq!(check(&["/callback"]), Err(RedirectUriError::Scheme));
        assert_eq!(
            check(&["example.com/callback"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(
            check(&["//example.com/callback"]),
            Err(RedirectUriError::Scheme)
        );
        assert_eq!(check(&[""]), Err(RedirectUriError::Scheme));
    }

    /// One bad URI is enough, which is what `Enum.all?/2` means.
    #[test]
    fn one_bad_uri_refuses_the_whole_list() {
        assert_eq!(
            check(&["https://example.com/cb", "ftp://example.com"]),
            Err(RedirectUriError::Scheme)
        );
    }

    #[test]
    fn an_empty_list_passes_vacuously() {
        assert_eq!(check(&[]), Ok(()));
    }

    /// `URI.parse/1` lowercases the scheme, and the Elixir compares against
    /// lowercase strings, so this passes there and has to pass here.
    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        assert_eq!(check(&["HTTPS://example.com/cb"]), Ok(()));
        assert_eq!(check(&["Http://example.com/cb"]), Ok(()));
    }

    /// Not a defence of anything, but it is what the Elixir does: it never looks
    /// past the scheme, so a URI this odd is acceptable to it.
    #[test]
    fn the_rest_of_the_uri_is_not_inspected() {
        assert_eq!(check(&["http:example"]), Ok(()));
    }

    fn scopes(scopes: &[&str]) -> Result<(), ScopeError> {
        check_scopes(
            &scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn the_only_acceptable_scopes_are_none_and_openid() {
        assert_eq!(scopes(&[]), Ok(()));
        assert_eq!(scopes(&["openid"]), Ok(()));
    }

    #[test]
    fn an_unknown_scope_is_refused() {
        assert_eq!(scopes(&["profile"]), Err(ScopeError::Invalid));
        assert_eq!(scopes(&["openid", "profile"]), Err(ScopeError::Invalid));
        assert_eq!(scopes(&[""]), Err(ScopeError::Invalid));
    }

    /// The half a caller would not think to check, and the reason the rule is
    /// two rules rather than a length.
    #[test]
    fn a_repeated_scope_is_refused() {
        assert_eq!(scopes(&["openid", "openid"]), Err(ScopeError::Invalid));
    }
}
