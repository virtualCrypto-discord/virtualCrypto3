//! Personal-delegation scopes. Legacy account JWT scopes are a separate vocabulary.
//! No wildcard, prefix matching, or implication between these permissions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    ProfileRead,
    BalancesRead,
    ClaimsRead,
    ContractsRead,
    ContractPaymentsRead,
    PaymentsCreate,
    ClaimsCreate,
    ClaimsApprove,
    ClaimsDeny,
    ClaimsCancel,
    ClaimsMetadataWrite,
}

impl Scope {
    pub const ALL: &'static [Self] = &[
        Self::ProfileRead,
        Self::BalancesRead,
        Self::ClaimsRead,
        Self::ContractsRead,
        Self::ContractPaymentsRead,
        Self::PaymentsCreate,
        Self::ClaimsCreate,
        Self::ClaimsApprove,
        Self::ClaimsDeny,
        Self::ClaimsCancel,
        Self::ClaimsMetadataWrite,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProfileRead => "vc.delegate.profile.read",
            Self::BalancesRead => "vc.delegate.balances.read",
            Self::ClaimsRead => "vc.delegate.claims.read",
            Self::ContractsRead => "vc.delegate.contracts.read",
            Self::ContractPaymentsRead => "vc.delegate.contracts.payments.read",
            Self::PaymentsCreate => "vc.delegate.payments.create",
            Self::ClaimsCreate => "vc.delegate.claims.create",
            Self::ClaimsApprove => "vc.delegate.claims.approve",
            Self::ClaimsDeny => "vc.delegate.claims.deny",
            Self::ClaimsCancel => "vc.delegate.claims.cancel",
            Self::ClaimsMetadataWrite => "vc.delegate.claims.metadata.write",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ProfileRead => "Read your profile",
            Self::BalancesRead => "Read your balances",
            Self::ClaimsRead => "Read your claims",
            Self::ContractsRead => "Read your contracts",
            Self::ContractPaymentsRead => "Read your contract payment history",
            Self::PaymentsCreate => "Send payments from your account",
            Self::ClaimsCreate => "Create claims addressed to other accounts",
            Self::ClaimsApprove => "Approve claims and pay from your account",
            Self::ClaimsDeny => "Deny incoming claims",
            Self::ClaimsCancel => "Cancel your outgoing claims",
            Self::ClaimsMetadataWrite => "Edit or delete your claim metadata",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scope| scope.as_str() == value)
    }
}
