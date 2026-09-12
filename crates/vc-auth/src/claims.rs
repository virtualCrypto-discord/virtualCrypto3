use serde::{Deserialize, Serialize};

pub const ISSUER: &str = "virtualCrypto";
pub const AUDIENCE: &str = "virtualCrypto";

pub const SCOPE_OAUTH2_REGISTER: &str = "oauth2.register";
pub const SCOPE_VC_PAY: &str = "vc.pay";
pub const SCOPE_VC_CLAIM: &str = "vc.claim";

/// The claim set Guardian emits. Field names and value types are part of the
/// contract with tokens already in the wild, so they must not change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: i64,
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
}

impl Scopes {
    pub fn from_list(scopes: &[String]) -> Self {
        let has = |scope: &str| scopes.iter().any(|candidate| candidate == scope);

        Self {
            oauth2_register: has(SCOPE_OAUTH2_REGISTER),
            vc_pay: has(SCOPE_VC_PAY),
            vc_claim: has(SCOPE_VC_CLAIM),
        }
    }
}
