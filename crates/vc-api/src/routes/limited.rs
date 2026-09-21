//! Operation-specific authorization for the account APIs.
//!
//! Authentication preserves the credential's authority: an account credential
//! and a personal delegation are different principals. Neither authentication
//! nor a delegation can be extracted by a handler directly. A handler must name
//! an operation, whose policy is checked before it receives an account id.

use std::marker::PhantomData;

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::{StatusCode, header::AUTHORIZATION, request::Parts};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;
use vc_auth::{AuthUser, Kind, Scopes};
use vc_core::grant::Target;

use crate::{error::ApiError, state::AppState};

/// Policies are sealed: new operations must be added to the policy table here.
pub trait Permission: private::Sealed + Send + Sync {
    const OPERATION: Operation;
}
mod private {
    pub trait Sealed {}
}

#[derive(Clone, Copy)]
pub enum Operation {
    Read,
    ReadClaims,
    Pay,
    WriteClaims,
    ApplicationContracts,
    DecideContract,
}

macro_rules! permissions {
    ($($name:ident => $operation:ident),* $(,)?) => {$(
        pub enum $name {}
        impl private::Sealed for $name {}
        impl Permission for $name {
            const OPERATION: Operation = Operation::$operation;
        }
    )*};
}
permissions! {
    Read => Read,
    ReadClaims => ReadClaims,
    Pay => Pay,
    WriteClaims => WriteClaims,
    ApplicationContracts => ApplicationContracts,
    DecideContract => DecideContract,
}

/// No default permission, no raw authentication extractor, and no `Deref` to
/// `AuthUser`: an operation must be chosen at every handler boundary.
pub struct Authorized<P: Permission> {
    principal: Principal,
    account_id: i32,
    permission: PhantomData<P>,
}

/// A delegation never becomes an account's own credential, even when both name
/// the same account. In particular, scopes cannot turn it into a contract party
/// credential or an application's credential.
enum Principal {
    Own(AuthUser),
    Delegated {
        account_id: i32,
        scopes: Scopes,
        resources: Vec<i64>,
    },
}

impl Principal {
    fn authorize(&self, operation: Operation) -> Result<(), ApiError> {
        use Operation::*;
        let (allowed, refusal) = match self {
            Self::Delegated { scopes, .. } => (
                match operation {
                    Read | ReadClaims => scopes.vc_read,
                    Pay => scopes.vc_pay,
                    WriteClaims => scopes.vc_claim,
                    ApplicationContracts | DecideContract => false,
                },
                match operation {
                    ApplicationContracts | DecideContract => ApiError::PermissionDenied,
                    _ => ApiError::InsufficientScope,
                },
            ),
            Self::Own(user) => (
                // Preserve the account credentials' existing API contract.
                match operation {
                    Read => true,
                    ReadClaims | WriteClaims => user.scopes.vc_claim,
                    Pay => user.scopes.vc_pay,
                    ApplicationContracts => user.kind == Kind::App && user.scopes.vc_contract,
                    DecideContract => user.kind == Kind::User,
                },
                match operation {
                    Pay => ApiError::InsufficientScope,
                    ApplicationContracts if user.kind == Kind::App => ApiError::InsufficientScope,
                    _ => ApiError::PermissionDenied,
                },
            ),
        };
        if allowed { Ok(()) } else { Err(refusal) }
    }
}

impl<P: Permission> Authorized<P> {
    pub fn account_id(&self) -> i32 {
        self.account_id
    }

    pub fn is_application(&self) -> bool {
        matches!(&self.principal, Principal::Own(user) if user.kind == Kind::App)
    }

    /// Empty is unrestricted. A delegation's nonempty list is retained even
    /// after its currencies are deleted; own credentials are unrestricted.
    pub fn resources(&self) -> &[i64] {
        match &self.principal {
            Principal::Own(_) => &[],
            Principal::Delegated { resources, .. } => resources,
        }
    }
}

impl<P: Permission> FromRequestParts<AppState> for Authorized<P> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let grant = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(vc_auth::extractor::bearer_token)
            .and_then(|token| Uuid::parse_str(token).ok());
        let principal = match grant {
            Some(token) => {
                let resolved =
                    vc_core::grant::resolve_token(state.pool(), token, OffsetDateTime::now_utc())
                        .await
                        .map_err(|error| ApiError::from(error).into_response())?;
                let Some(resolved) =
                    resolved.filter(|grant| matches!(grant.target, Target::User(_)))
                else {
                    return Err((
                        StatusCode::UNAUTHORIZED,
                        Json(json!({"error":"invalid_token"})),
                    )
                        .into_response());
                };
                Principal::Delegated {
                    account_id: resolved.account_id,
                    scopes: Scopes::from_list(&resolved.scopes),
                    resources: resolved.resources,
                }
            }
            None => Principal::Own(
                AuthUser::from_request_parts(parts, state)
                    .await
                    .map_err(IntoResponse::into_response)?,
            ),
        };
        let account_id = match &principal {
            Principal::Own(user) => i32::try_from(user.subject)
                .map_err(|_| ApiError::Internal("subject out of range".into()).into_response())?,
            Principal::Delegated { account_id, .. } => *account_id,
        };
        if !state.limiter().allow(&format!("v2:{account_id}")) {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error":"rate_limited"})),
            )
                .into_response());
        }
        principal
            .authorize(P::OPERATION)
            .map_err(IntoResponse::into_response)?;
        Ok(Self {
            principal,
            account_id,
            permission: PhantomData,
        })
    }
}
