//! The consent screen.
//!
//! What is here so far is the part of it worth getting wrong-proof separately:
//! how a refusal is answered. The rest of the screen is a validation chain, and
//! each link of it lands on one of these.

use vc_core::application::PreauthorizeError;

/// What to do about a refusal.
///
/// The Elixir is asymmetric here, and deliberately so rather than by accident: a
/// bad `client_id` or a `redirect_uri` that is not the application's own cannot
/// be answered with a redirect, because the redirect target is exactly what has
/// not been established as safe to send a browser to. Everything else is answered
/// by sending the browser back to the client, which is what an OAuth2 client is
/// listening for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Answer the browser here, and send it nowhere.
    Page,
    /// Send it back to the client with this error.
    Redirect {
        error: &'static str,
        description: &'static str,
    },
}

impl Refusal {
    /// A refusal the caller raises itself, all of which redirect: by the time
    /// these are reached the client and the redirect URI have both been checked.
    pub fn redirect(error: &'static str, description: &'static str) -> Self {
        Refusal::Redirect { error, description }
    }
}

/// How `preauthorize`'s four answers are answered.
pub fn refusal_for(error: PreauthorizeError) -> Refusal {
    match error {
        // The two that must not redirect.
        PreauthorizeError::InvalidClientId | PreauthorizeError::InvalidRedirectUri => Refusal::Page,

        PreauthorizeError::InvalidScope => Refusal::redirect("invalid_request", "invalid_scope"),

        PreauthorizeError::InvalidApplicationGrantType => {
            Refusal::redirect("unauthorized_client", "invalid_application_grant_type")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the distinction: a redirect_uri that is not the
    /// application's own is not a place to send a browser, so the refusal must
    /// stay here.
    #[test]
    fn an_untrusted_redirect_uri_is_not_redirected_to() {
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidRedirectUri),
            Refusal::Page
        );
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidClientId),
            Refusal::Page
        );
    }

    /// Once both of those have been checked, a refusal is safe to deliver to the
    /// client — which is the only way it learns what went wrong.
    #[test]
    fn the_rest_go_back_to_the_client() {
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidScope),
            Refusal::Redirect {
                error: "invalid_request",
                description: "invalid_scope",
            }
        );
        assert_eq!(
            refusal_for(PreauthorizeError::InvalidApplicationGrantType),
            Refusal::Redirect {
                error: "unauthorized_client",
                description: "invalid_application_grant_type",
            }
        );
    }

    #[test]
    fn a_refusal_it_raises_itself_redirects() {
        assert_eq!(
            Refusal::redirect("invalid_request", "invalid_guild_id"),
            Refusal::Redirect {
                error: "invalid_request",
                description: "invalid_guild_id",
            }
        );
    }
}
