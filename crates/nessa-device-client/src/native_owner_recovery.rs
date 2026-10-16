//! Immutable foreign original recovery, local settlement and exact failure facts.
use crate::{
    failures::{NativeEnrollmentFailure, NativePrivateStateFailure, NativeReadFailure},
    native_owner_phone::{NativeOwnerPhonePreparation, NativeOwnerPreparationFailure},
    native_session::{NativeEnrollmentReceipt, NativePublicIntent, NativePurge},
};
use nessa_auth::application::pairing::{
    OwnerPhoneAdoption, OwnerPhoneAdoptionFailure, OwnerPhoneAdoptionPhase,
    OwnerPhoneEnrollmentKind,
};
use nessa_client_core::{
    pairing::{
        OwnerPhoneRecovered, OwnerPhoneRecoveryFailure, OwnerPhoneSettlement,
        OwnerPhoneSettlementFailure,
    },
    retained::RetainedOwnerStatus,
};
use std::sync::Arc;

/// Original recovery refusal family; it does not establish absence of effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerRecoveryFailureKind {
    /// Canonical preparation or private acknowledgement refused.
    Preparation,
    /// Physical admission, TLS, compact exchange or worker refused.
    Native,
}
/// Original failure payload, with public facts only and no parent ownership.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerRecoveryFailure {
    pub(crate) value: OwnerPhoneRecoveryFailure,
}
#[uniffi::export]
impl NativeOwnerRecoveryFailure {
    /// Exact original family.
    pub fn kind(&self) -> NativeOwnerRecoveryFailureKind {
        match self.value {
            OwnerPhoneRecoveryFailure::Preparation(_) => {
                NativeOwnerRecoveryFailureKind::Preparation
            }
            OwnerPhoneRecoveryFailure::Native(_) => NativeOwnerRecoveryFailureKind::Native,
        }
    }
    /// Exact preparation failure, including actual publication/reconciliation payloads.
    pub fn preparation(&self) -> Option<Arc<NativeOwnerPreparationFailure>> {
        match &self.value {
            OwnerPhoneRecoveryFailure::Preparation(value) => {
                Some(Arc::new(NativeOwnerPreparationFailure {
                    value: value.clone(),
                }))
            }
            _ => None,
        }
    }
    /// Original physical/handshake/wire/worker payload.
    pub fn native(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        match self.value {
            OwnerPhoneRecoveryFailure::Native(value) => {
                Some(Arc::new(NativeEnrollmentFailure { value }))
            }
            _ => None,
        }
    }
}

/// Public preparation and the actual pinned compact status from one operation.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerRecovered {
    value: OwnerPhoneRecovered,
}
#[uniffi::export]
impl NativeOwnerRecovered {
    /// Historical public proof, not current private-key possession or approval.
    pub fn preparation(&self) -> Arc<NativeOwnerPhonePreparation> {
        Arc::new(NativeOwnerPhonePreparation {
            value: self.value.preparation().clone(),
        })
    }
    /// Actual historical pinned status, retained even when subsequent adoption failed.
    pub fn receipt(&self) -> Arc<NativeEnrollmentReceipt> {
        Arc::new(NativeEnrollmentReceipt {
            value: self.value.status().clone(),
        })
    }
}

/// Historical kind acknowledged by the local Auth owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerEnrollmentKind {
    /// Exact lineage without issued selectors.
    Pending,
    /// Exact lineage and issued credential/receiver.
    Issued,
}
/// Immutable storage receipt; callers cannot supply it as live authority.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerAdoption {
    value: OwnerPhoneAdoption,
}
#[uniffi::export]
impl NativeOwnerAdoption {
    /// Exact reported public reference, including all original lineage fields.
    pub fn public_intent(&self) -> Arc<NativePublicIntent> {
        Arc::new(NativePublicIntent {
            value: self.value.public(),
        })
    }
    /// Historical stored record kind.
    pub fn kind(&self) -> NativeOwnerEnrollmentKind {
        match self.value.kind() {
            OwnerPhoneEnrollmentKind::Pending => NativeOwnerEnrollmentKind::Pending,
            OwnerPhoneEnrollmentKind::Issued => NativeOwnerEnrollmentKind::Issued,
        }
    }
}
/// Auth's finite first/second acknowledgement failure phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerAdoptionPhase {
    /// Validation/read refused before lineage acknowledgement.
    OriginalRead,
    /// Lineage acknowledgement did not complete.
    BindLineage,
    /// Lineage acknowledgement completed; enrollment acknowledgement refused.
    PublishEnrollment,
}
/// Original local failure and separately completed earlier acknowledgement.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerAdoptionFailure {
    value: OwnerPhoneAdoptionFailure,
}
#[uniffi::export]
impl NativeOwnerAdoptionFailure {
    /// Exact original owner phase.
    pub fn phase(&self) -> NativeOwnerAdoptionPhase {
        match self.value.phase() {
            OwnerPhoneAdoptionPhase::OriginalRead => NativeOwnerAdoptionPhase::OriginalRead,
            OwnerPhoneAdoptionPhase::BindLineage => NativeOwnerAdoptionPhase::BindLineage,
            OwnerPhoneAdoptionPhase::PublishEnrollment => {
                NativeOwnerAdoptionPhase::PublishEnrollment
            }
        }
    }
    /// Unchanged publication step, effect, primary, cleanup and reconciliation payload.
    pub fn private_state(&self) -> Arc<NativePrivateStateFailure> {
        Arc::new(NativePrivateStateFailure::from_state(self.value.error()))
    }
    /// Independently completed lineage acknowledgement, absent in earlier phases.
    pub fn acknowledged_lineage(&self) -> Option<Arc<NativePublicIntent>> {
        self.value
            .acknowledged_lineage()
            .map(|value| Arc::new(NativePublicIntent { value }))
    }
}

/// A local settlement/selection failure preserves its own exact owner family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerSettlementFailureKind {
    /// Restored current original refused before settlement.
    Preparation,
    /// Actual Auth adoption refused.
    Adoption,
    /// Reported success contradicted actual receipt or saved selection.
    SavedRecord,
    /// Exact terminal private removal refused.
    End,
    /// Current terminal record contradicted the original observation.
    TerminalRecord,
    /// Current bound lineage contradicted the owned recovery.
    Correlation,
}
/// Independent later failure; retaining it never erases the recovered receipt.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerSettlementFailure {
    value: OwnerPhoneSettlementFailure,
}
#[uniffi::export]
impl NativeOwnerSettlementFailure {
    /// Original family.
    pub fn kind(&self) -> NativeOwnerSettlementFailureKind {
        match self.value {
            OwnerPhoneSettlementFailure::Preparation(_) => {
                NativeOwnerSettlementFailureKind::Preparation
            }
            OwnerPhoneSettlementFailure::Adoption(_) => NativeOwnerSettlementFailureKind::Adoption,
            OwnerPhoneSettlementFailure::SavedRecord { .. } => {
                NativeOwnerSettlementFailureKind::SavedRecord
            }
            OwnerPhoneSettlementFailure::End(_) => NativeOwnerSettlementFailureKind::End,
            OwnerPhoneSettlementFailure::TerminalRecord(_) => {
                NativeOwnerSettlementFailureKind::TerminalRecord
            }
            OwnerPhoneSettlementFailure::Correlation(_) => {
                NativeOwnerSettlementFailureKind::Correlation
            }
        }
    }
    /// Original preparation error, including current private-state refusal.
    pub fn preparation(&self) -> Option<Arc<NativeOwnerPreparationFailure>> {
        match &self.value {
            OwnerPhoneSettlementFailure::Preparation(value) => {
                Some(Arc::new(NativeOwnerPreparationFailure {
                    value: value.clone(),
                }))
            }
            _ => None,
        }
    }
    /// Original Auth adoption failure and independent lineage fact.
    pub fn adoption(&self) -> Option<Arc<NativeOwnerAdoptionFailure>> {
        match self.value {
            OwnerPhoneSettlementFailure::Adoption(value) => {
                Some(Arc::new(NativeOwnerAdoptionFailure { value }))
            }
            _ => None,
        }
    }
    /// The producer's nominal receipt retained beside its subsequent rejection.
    pub fn reported_adoption(&self) -> Option<Arc<NativeOwnerAdoption>> {
        match self.value {
            OwnerPhoneSettlementFailure::SavedRecord { reported, .. } => {
                Some(Arc::new(NativeOwnerAdoption { value: reported }))
            }
            _ => None,
        }
    }
    /// Exact subsequent original/native saved-record refusal.
    pub fn selection(&self) -> Option<Arc<NativeOwnerRecoveryFailure>> {
        match &self.value {
            OwnerPhoneSettlementFailure::SavedRecord { error, .. } => {
                Some(Arc::new(NativeOwnerRecoveryFailure {
                    value: error.clone(),
                }))
            }
            _ => None,
        }
    }
    /// Original authenticated terminal cleanup's private failure.
    pub fn end(&self) -> Option<Arc<NativePrivateStateFailure>> {
        match self.value {
            OwnerPhoneSettlementFailure::End(value) => {
                Some(Arc::new(NativePrivateStateFailure::from_state(value)))
            }
            _ => None,
        }
    }
    /// Original terminal-record or bound-lineage correlation refusal.
    pub fn correlation(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        match self.value {
            OwnerPhoneSettlementFailure::TerminalRecord(value)
            | OwnerPhoneSettlementFailure::Correlation(value) => {
                Some(Arc::new(NativeEnrollmentFailure { value }))
            }
            _ => None,
        }
    }
}
/// Historical completed settlement kind, never a current permission flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeOwnerSettlementKind {
    /// Exact local enrollment acknowledged and correlated to actual saved data.
    Adopted,
    /// Exact authenticated terminal enrollment ended.
    Ended,
}
/// One immutable original operation, including outer unconfirmed completion.
#[derive(Debug, uniffi::Object)]
pub struct NativeOwnerStatus {
    pub(crate) value: Result<RetainedOwnerStatus, OwnerPhoneRecoveryFailure>,
}
#[uniffi::export]
impl NativeOwnerStatus {
    /// Whether the physical callback captured a completed aggregate.
    pub fn completion_captured(&self) -> bool {
        self.value.is_ok()
    }
    /// Actual pinned observation, independently of settlement success.
    pub fn recovered(&self) -> Option<Arc<NativeOwnerRecovered>> {
        let value = self.value.as_ref().ok()?.recovery().ok()?;
        Some(Arc::new(NativeOwnerRecovered {
            value: value.clone(),
        }))
    }
    /// Original outer or captured recovery failure; no effects are inferred absent.
    pub fn recovery_failure(&self) -> Option<Arc<NativeOwnerRecoveryFailure>> {
        let value = match &self.value {
            Err(value) => Some(value),
            Ok(value) => value.recovery().err(),
        }?;
        Some(Arc::new(NativeOwnerRecoveryFailure {
            value: value.clone(),
        }))
    }
    /// Actual successful historical settlement, absent for recovery-only or refusal.
    pub fn settlement_kind(&self) -> Option<NativeOwnerSettlementKind> {
        match self.value.as_ref().ok()?.settlement()?.as_ref().ok()? {
            OwnerPhoneSettlement::Adopted(_) => Some(NativeOwnerSettlementKind::Adopted),
            OwnerPhoneSettlement::Ended => Some(NativeOwnerSettlementKind::Ended),
        }
    }
    /// Correlated local adoption receipt, not live product authority.
    pub fn adoption(&self) -> Option<Arc<NativeOwnerAdoption>> {
        match self.value.as_ref().ok()?.settlement()?.as_ref().ok()? {
            OwnerPhoneSettlement::Adopted(value) => {
                Some(Arc::new(NativeOwnerAdoption { value: *value }))
            }
            OwnerPhoneSettlement::Ended => None,
        }
    }
    /// Exact later settlement refusal, independently of successful remote observation.
    pub fn settlement_failure(&self) -> Option<Arc<NativeOwnerSettlementFailure>> {
        let value = self.value.as_ref().ok()?.settlement()?.as_ref().err()?;
        Some(Arc::new(NativeOwnerSettlementFailure {
            value: value.clone(),
        }))
    }
    /// Original successful purge, including when later credential removal failed.
    pub fn purge(&self) -> Option<Arc<NativePurge>> {
        let value = self.value.as_ref().ok()?.purge()?.as_ref().ok()?;
        Some(Arc::new(NativePurge {
            value: value.clone(),
        }))
    }
    /// Original purge refusal, without a permission or deletion inference.
    pub fn purge_failure(&self) -> Option<NativeReadFailure> {
        self.value
            .as_ref()
            .ok()?
            .purge()?
            .as_ref()
            .err()
            .copied()
            .map(Into::into)
    }
}
