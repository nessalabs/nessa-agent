//! Run one cut: hold admission while each store is pinned and fences are
//! appended, then release it and read, mac, and publish from those snapshots.

mod deletion_inventory;
mod export;
mod ports;
mod verification;
mod verification_format;

pub use verification_format::{
    DecodedManifest, DeletionReadError, ManifestReadError, VerificationFormat,
};

pub use verification::{
    verify_bundle, InventoryVerification, ManifestMacVerifier, VerificationFailure,
    VerificationLimits, VerificationReport, VerificationSource, VerifiedOmission,
};

pub use deletion_inventory::{capture_fences, capture_inventory, CaptureError, InventoryFailure};
pub use export::{capture_export, ExportFailure, ExportRequest};
pub use ports::{
    AdmissionFailure, AdmissionLease, Authenticator, CopiedStore, DecisionAuditError,
    DeletionLogStore, ExportClock, ExportSource, FenceSource, InventoryAction,
    InventoryCapturePolicyEvaluator, InventoryCause, InventoryDecision, InventoryDecisionAudit,
    InventoryDenial, InventoryFacts, InventoryReconciliationLease, LogStoreError, MacError, MacKey,
    ManifestPublisher, MutationAdmission, PinnedStore, PublishedRecord,
};
