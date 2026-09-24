//! Validated caller, correlation, target, and outcome facts for credential saves.
mod credential_save_evidence;
#[cfg(test)]
pub use credential_save_evidence::CredentialSaveTargetError;
pub use credential_save_evidence::{
    CredentialSaveCaller, CredentialSaveCorrelation, CredentialSaveEffect, CredentialSaveIntent,
    CredentialSaveOutcome, CredentialSaveRefusal, CredentialSaveTarget, CredentialSaveUncertainty,
};
