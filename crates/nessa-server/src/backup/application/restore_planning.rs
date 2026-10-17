//! Inspect a verified cut without acquiring any import or activation capability.

use serde::Serialize;

use super::{
    verify_bundle, ManifestMacVerifier, VerificationFailure, VerificationLimits,
    VerificationReport, VerificationSource,
};
use crate::backup::domain::WRITER_INVENTORY;

/// A report about the bytes inspected, never a permit or an executable plan.
/// Private fields prevent callers from constructing an allegedly accepted plan.
#[derive(Debug, Serialize)]
pub struct OfflineRestorePlan {
    verification: VerificationReport,
    omission_summary: Vec<StoreOmissions>,
    activation: Activation,
    unresolved_prerequisites: Vec<Prerequisite>,
    sequence_status: SequenceStatus,
    proposed_sequence: [PlanningStep; 6],
    requires_reverification: bool,
}

#[derive(Debug, Serialize)]
struct StoreOmissions {
    store: &'static str,
    rows: usize,
    redacted: usize,
    skipped: usize,
    deferred: usize,
    source_origins: SourceOrigins,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Activation {
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceOrigins {
    NotEstablished,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Prerequisite {
    TrustedKeyRecovery,
    IndependentInventoryFreshnessAndRollbackProof,
    ReplacementIdentityPolicy,
    OldSourceAndRemoteExecutorFencing,
    OmissionAcceptancePolicy,
    StoreOwnerPayloadCompatibility,
    QuarantineImplementation,
    StoreOwnerImportImplementation,
    AuditedAuthorityAcceptanceImplementation,
    DeferredPayloadAvailability,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SequenceStatus {
    ProposalUnimplemented,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlanningStep {
    SelectNewPrivateDestination,
    PersistQuarantineBeforeAnyConsumers,
    ValidateAndImportThroughStoreOwners,
    ReconcileIndependentlyTrustedInventory,
    EstablishChosenIdentityAndProveFencing,
    ImplementExplicitAuditedAuthorityAcceptance,
}

/// Verify with the existing bounded reader, then describe unresolved work.
/// No destination, gateway, identity, network, writer or execution port exists.
/// Even a complete, empty, authenticated cut cannot produce activation authority.
pub fn plan_offline_restore(
    source: &dyn VerificationSource,
    verifier: &dyn ManifestMacVerifier,
    limits: VerificationLimits,
) -> Result<OfflineRestorePlan, VerificationFailure> {
    let verification = verify_bundle(source, verifier, limits)?;
    let omission_summary = WRITER_INVENTORY
        .iter()
        .filter_map(|store| {
            let rows = verification
                .omissions
                .iter()
                .filter(|row| row.store == store.as_str())
                .collect::<Vec<_>>();
            (!rows.is_empty()).then(|| StoreOmissions {
                store: store.as_str(),
                rows: rows.len(),
                redacted: rows.iter().filter(|row| row.kind == "redacted").count(),
                skipped: rows.iter().filter(|row| row.kind == "skipped").count(),
                deferred: rows.iter().filter(|row| row.kind == "deferred").count(),
                source_origins: SourceOrigins::NotEstablished,
            })
        })
        .collect();
    let mut unresolved_prerequisites = vec![
        Prerequisite::TrustedKeyRecovery,
        Prerequisite::IndependentInventoryFreshnessAndRollbackProof,
        Prerequisite::ReplacementIdentityPolicy,
        Prerequisite::OldSourceAndRemoteExecutorFencing,
        Prerequisite::OmissionAcceptancePolicy,
        Prerequisite::StoreOwnerPayloadCompatibility,
        Prerequisite::QuarantineImplementation,
        Prerequisite::StoreOwnerImportImplementation,
        Prerequisite::AuditedAuthorityAcceptanceImplementation,
    ];
    if verification
        .omissions
        .iter()
        .any(|row| row.kind == "deferred")
    {
        unresolved_prerequisites.push(Prerequisite::DeferredPayloadAvailability);
    }
    Ok(OfflineRestorePlan {
        verification,
        omission_summary,
        activation: Activation::Unavailable,
        unresolved_prerequisites,
        sequence_status: SequenceStatus::ProposalUnimplemented,
        proposed_sequence: [
            PlanningStep::SelectNewPrivateDestination,
            PlanningStep::PersistQuarantineBeforeAnyConsumers,
            PlanningStep::ValidateAndImportThroughStoreOwners,
            PlanningStep::ReconcileIndependentlyTrustedInventory,
            PlanningStep::EstablishChosenIdentityAndProveFencing,
            PlanningStep::ImplementExplicitAuditedAuthorityAcceptance,
        ],
        requires_reverification: true,
    })
}
