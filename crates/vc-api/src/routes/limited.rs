//! Operation-specific authorization for the account APIs.
//!
//! Authentication preserves the credential's authority: an account credential
//! and a personal delegation are different principals. Neither authentication
//! nor a delegation can be extracted by a handler directly. A handler must name
//! an operation, whose policy is checked before it receives an account id.

use std::marker::PhantomData;

use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::{StatusCode, header::AUTHORIZATION, request::Parts};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;
use vc_auth::{AuthUser, Kind};
use vc_core::claim::Transition;
use vc_core::delegation::Scope;
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
    ReadProfile,
    ReadBalances,
    ReadContracts,
    ReadContractPayments,
    ReadClaims,
    Pay,
    CreateClaims,
    PatchClaims,
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
    ReadProfile => ReadProfile,
    ReadBalances => ReadBalances,
    ReadContracts => ReadContracts,
    ReadContractPayments => ReadContractPayments,
    ReadClaims => ReadClaims,
    Pay => Pay,
    CreateClaims => CreateClaims,
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
        scopes: Vec<Scope>,
        resources: Vec<i64>,
    },
}

impl Principal {
    fn authorize(&self, operation: Operation) -> Result<(), ApiError> {
        use Operation::*;
        let (allowed, refusal) = match self {
            Self::Delegated { scopes, .. } => (
                match operation {
                    ReadProfile => scopes.contains(&Scope::ProfileRead),
                    ReadBalances => scopes.contains(&Scope::BalancesRead),
                    ReadContracts => scopes.contains(&Scope::ContractsRead),
                    ReadContractPayments => scopes.contains(&Scope::ContractPaymentsRead),
                    ReadClaims => scopes.contains(&Scope::ClaimsRead),
                    Pay => scopes.contains(&Scope::PaymentsCreate),
                    CreateClaims => scopes.contains(&Scope::ClaimsCreate),
                    PatchClaims => scopes.iter().any(|scope| {
                        matches!(
                            scope,
                            Scope::ClaimsApprove
                                | Scope::ClaimsDeny
                                | Scope::ClaimsCancel
                                | Scope::ClaimsMetadataWrite
                        )
                    }),
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
                    ReadProfile | ReadBalances | ReadContracts | ReadContractPayments => true,
                    ReadClaims | CreateClaims | PatchClaims => user.scopes.vc_claim,
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
                    scopes: resolved
                        .scopes
                        .iter()
                        .filter_map(|scope| Scope::parse(scope))
                        .collect(),
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

// Only ClaimPatch can extract this broad family. Handlers receive a body whose
// exact operation and any accompanying metadata write have already been authorized.
enum PatchClaims {}
impl private::Sealed for PatchClaims {}
impl Permission for PatchClaims {
    const OPERATION: Operation = Operation::PatchClaims;
}

pub struct ClaimPatch {
    user: Authorized<PatchClaims>,
    body: Value,
    transition: Option<Transition>,
}

impl ClaimPatch {
    pub fn account_id(&self) -> i32 {
        self.user.account_id()
    }
    pub fn resources(&self) -> &[i64] {
        self.user.resources()
    }
    pub fn body(&self) -> &Value {
        &self.body
    }
    pub fn transition(&self) -> Option<Transition> {
        self.transition
    }
}

impl FromRequest<AppState> for ClaimPatch {
    type Rejection = Response;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = request.into_parts();
        let user = Authorized::<PatchClaims>::from_request_parts(&mut parts, state).await?;
        let Json(body) = Json::<Value>::from_request(Request::from_parts(parts, body), state)
            .await
            .map_err(IntoResponse::into_response)?;
        let transition = match body.get("status").and_then(Value::as_str) {
            Some("approved") => Some(Transition::Approved),
            Some("denied") => Some(Transition::Denied),
            Some("canceled") => Some(Transition::Canceled),
            _ => None,
        };
        if let Principal::Delegated { scopes, .. } = &user.principal {
            let needed = transition.map(|transition| match transition {
                Transition::Approved => Scope::ClaimsApprove,
                Transition::Denied => Scope::ClaimsDeny,
                Transition::Canceled => Scope::ClaimsCancel,
            });
            let metadata = body
                .as_object()
                .is_some_and(|object| object.contains_key("metadata"));
            if needed.is_some_and(|scope| !scopes.contains(&scope))
                || (metadata && !scopes.contains(&Scope::ClaimsMetadataWrite))
            {
                return Err(ApiError::InsufficientScope.into_response());
            }
        }
        Ok(Self {
            user,
            body,
            transition,
        })
    }
}
