//! Run one cut: hold admission while each store is pinned and fences are
//! appended, then release it and read, mac, and publish from those snapshots.

mod deletion_inventory;
mod export;
mod ports;
mod restore;
mod restore_planning;
mod verification;
mod verification_format;

pub use verification_format::{
    DecodedManifest, DeletionReadError, ManifestReadError, VerificationFormat,
};

pub use restore_planning::{plan_offline_restore, OfflineRestorePlan};

pub use verification::{
    authenticated_bundle, verify_bundle, AuthenticatedBundle, InventoryVerification,
    ManifestMacVerifier, VerificationFailure, VerificationLimits, VerificationReport,
    VerificationSource, VerifiedOmission,
};

pub use deletion_inventory::{capture_fences, capture_inventory, CaptureError, InventoryFailure};
pub use export::{capture_export, ExportFailure, ExportRequest};
pub use ports::{
    current_staging_id, effect_names_seal, fetch_names_conversation, AdmissionFailure,
    AdmissionLease, Authenticator, CopiedStore, DecisionAuditError, DeferredSessionSource,
    DeletionLogStore, ExportClock, ExportSource, FenceSource, FetchProgress, InventoryAction,
    InventoryCapturePolicyEvaluator, InventoryCause, InventoryDecision, InventoryDecisionAudit,
    InventoryDenial, InventoryFacts, InventoryReconciliationLease, LeaseReceipts, LogStoreError,
    MacError, MacKey, ManifestPublisher, MutationAdmission, PinnedStore, PublicationIds,
    PublishedRecord, RestoreAction, RestoreAudit, RestoreEvidence, RestoreFacts,
    RestorePolicyEvaluator, SealFields, SessionCut, SessionSourceError, StagingIds,
};
#[cfg(test)]
pub(crate) use restore::ordered_roots;
pub use restore::{
    accept_quarantine, open_deferred_conversation, root_posture, stage_quarantine, AcceptReport,
    AcceptRequest, AnchoredRoot, CutMaterializer, LoadedTree, OpenReport, OpenRequest, PreparedCut,
    Promoted, QuarantineLayout, RestoreHold, SealedInventoryReader, StageReport, StageRequest,
    Staging, TreeId,
};
