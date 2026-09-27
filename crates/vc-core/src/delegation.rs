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
            Self::ProfileRead => "あなたのプロフィールを閲覧する",
            Self::BalancesRead => "あなたの残高を閲覧する",
            Self::ClaimsRead => "あなたの請求を閲覧する",
            Self::ContractsRead => "あなたの契約を閲覧する",
            Self::ContractPaymentsRead => "あなたの契約の支払い履歴を閲覧する",
            Self::PaymentsCreate => "あなたのアカウントから送金する",
            Self::ClaimsCreate => "あなたからの請求を作成・閲覧する",
            Self::ClaimsApprove => "あなた宛ての請求を閲覧・承認し、あなたのアカウントから支払う",
            Self::ClaimsDeny => "あなた宛ての請求を閲覧・拒否する",
            Self::ClaimsCancel => "あなたからの請求を閲覧・取り消す",
            Self::ClaimsMetadataWrite => "あなたの請求を閲覧し、そのメタデータを編集・削除する",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|scope| scope.as_str() == value)
    }
}
