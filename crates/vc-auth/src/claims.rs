use serde::{Deserialize, Serialize};

pub const ISSUER: &str = "virtualCrypto";
pub const AUDIENCE: &str = "virtualCrypto";

pub const SCOPE_OAUTH2_REGISTER: &str = "oauth2.register";
pub const SCOPE_VC_PAY: &str = "vc.pay";
pub const SCOPE_VC_CLAIM: &str = "vc.claim";
/// The scope a guild grants an application so that it may issue from the guild's
/// pool. It is carried by a guild token and by nothing else: an `app` token that
/// asked for it would have no guild to issue in.
pub const SCOPE_VC_ISSUE: &str = "vc.issue";
/// The scope an application carries to write contracts: it is the right to ask
/// users to lock their currency with it, and nothing more — what lets it
/// actually spend is each user's own approval.
pub const SCOPE_VC_CONTRACT: &str = "vc.contract";

/// The claim set Guardian emits. Field names and value types are part of the
/// contract with tokens already in the wild, so they must not change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    /// When the token stops being accepted, or absent for one that lives until it is revoked.
    ///
    /// Guardian always wrote it, and every token this service issued before personal access tokens
    /// has one. A token without one is a credential something that is not a browser keeps: it has
    /// no session to renew from and nobody watching it, so the `DELETE` that revocation is — and
    /// nothing on a clock — is what ends it. The claim is left out rather than written as `null`,
    /// because a verifier that reads JSON has to see a number or nothing at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub nbf: Option<i64>,
    pub iss: String,
    #[serde(default)]
    pub aud: Option<String>,
    pub jti: String,
    pub kind: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub typ: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    User,
    App,
}

impl Kind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Kind::User),
            "app" => Some(Kind::App),
            _ => None,
        }
    }
}

/// The boolean scope flags Guardian exposes as the loaded resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scopes {
    pub oauth2_register: bool,
    pub vc_pay: bool,
    pub vc_claim: bool,
    /// Read from the same claim as the rest, and carried by a guild token.
    pub vc_issue: bool,
    /// Carried by an `app` token, like `vc_pay` and `vc_claim`.
    pub vc_contract: bool,
}

impl Scopes {
    pub fn from_list(scopes: &[String]) -> Self {
        let has = |scope: &str| scopes.iter().any(|candidate| candidate == scope);

        Self {
            oauth2_register: has(SCOPE_OAUTH2_REGISTER),
            vc_pay: has(SCOPE_VC_PAY),
            vc_claim: has(SCOPE_VC_CLAIM),
            vc_issue: has(SCOPE_VC_ISSUE),
            vc_contract: has(SCOPE_VC_CONTRACT),
        }
    }
}
