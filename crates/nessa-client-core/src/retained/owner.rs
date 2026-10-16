//! Immutable per-operation original observation and independent settlement facts.
use crate::pairing::owner_recovery::{
    OwnerPhoneRecovered, OwnerPhoneRecoveryFailure, OwnerPhoneSettlement,
    OwnerPhoneSettlementFailure,
};
use crate::retained::{ReadFailure, RetainedPurge};

// A failed recovery has no settlement/purge slot. Only an actual recovered value
// can construct Completed and reach the existing retained settlement callback.
#[derive(Clone, Debug, PartialEq, Eq)]
enum OwnerOperation {
    RecoveryFailed(OwnerPhoneRecoveryFailure),
    Completed(Box<OwnerCompletion>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnerCompletion {
    recovered: OwnerPhoneRecovered,
    settlement: Option<Result<OwnerPhoneSettlement, OwnerPhoneSettlementFailure>>,
    purge: Option<Result<RetainedPurge, ReadFailure>>,
}

/// One completed physical operation's facts, with no private owner or authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedOwnerStatus {
    operation: OwnerOperation,
}
impl RetainedOwnerStatus {
    // Only retained composition constructs either branch, while still admitted.
    pub(super) fn recovery_failed(failure: OwnerPhoneRecoveryFailure) -> Self {
        Self {
            operation: OwnerOperation::RecoveryFailed(failure),
        }
    }
    pub(super) fn recovered(
        recovered: OwnerPhoneRecovered,
        settlement: Option<Result<OwnerPhoneSettlement, OwnerPhoneSettlementFailure>>,
        purge: Option<Result<RetainedPurge, ReadFailure>>,
    ) -> Self {
        Self {
            operation: OwnerOperation::Completed(Box::new(OwnerCompletion {
                recovered,
                settlement,
                purge,
            })),
        }
    }
    /// Exact observed recovery or original producer/native refusal.
    pub fn recovery(&self) -> Result<&OwnerPhoneRecovered, &OwnerPhoneRecoveryFailure> {
        match &self.operation {
            OwnerOperation::RecoveryFailed(failure) => Err(failure),
            OwnerOperation::Completed(completed) => Ok(&completed.recovered),
        }
    }
    /// This operation's settlement, only possible after a successful recovery.
    pub fn settlement(&self) -> Option<&Result<OwnerPhoneSettlement, OwnerPhoneSettlementFailure>> {
        match &self.operation {
            OwnerOperation::RecoveryFailed(_) => None,
            OwnerOperation::Completed(completed) => completed.settlement.as_ref(),
        }
    }
    /// This physical operation's actual purge effect, distinct from durable history.
    pub fn purge(&self) -> Option<&Result<RetainedPurge, ReadFailure>> {
        match &self.operation {
            OwnerOperation::RecoveryFailed(_) => None,
            OwnerOperation::Completed(completed) => completed.purge.as_ref(),
        }
    }
}

impl RetainedOwnerStatus {
    /// An operation may select a fresh reader only after correlated local settlement.
    /// This borrowed status is historical; the gateway admits subsequent requests.
    pub fn status_if_settled(&self) -> Option<&nessa_protocol::pairing::wire::NativePairingStatus> {
        let OwnerOperation::Completed(value) = &self.operation else {
            return None;
        };
        value.settlement.as_ref()?.as_ref().ok()?;
        Some(value.recovered.status())
    }
}
