//! Personal-delegation scopes. Legacy account JWT scopes are a separate vocabulary.
//! Names match exactly. Claim writes also allow reading the claims they act on.

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
            Self::ClaimsCreate => "Create and read your outgoing claims",
            Self::ClaimsApprove => "Read incoming claims and approve them to pay from your account",
            Self::ClaimsDeny => "Read and deny incoming claims",
            Self::ClaimsCancel => "Read and cancel your outgoing claims",
            Self::ClaimsMetadataWrite => "Read your claims and edit or delete their metadata",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scope| scope.as_str() == value)
    }
}
