//! Native foreign adapter over the shared device core. Foreign methods are
//! synchronous: caller composition dispatches them off its UI thread and owns
//! admission and scope replacement. Generated immutable objects retain saved
//! evidence; they do not grant authority or become fresh after a later read.
#![deny(missing_docs)]
mod failures;
mod infrastructure;
mod native_owner_phone;
mod native_owner_recovery;
mod native_product;
mod native_session;
mod saved;
mod serialization;
mod session;
#[cfg(test)]
#[path = "../tests/support/private_directory.rs"]
mod test_support;
uniffi::setup_scaffolding!();
pub use failures::*;
pub use native_owner_phone::*;
pub use native_owner_recovery::*;
pub use native_product::*;
pub use native_session::*;
pub use saved::{
    NativeSavedCache, NativeScope, NativeTranscript, NativeTranscriptState, NativeViewError,
};
pub use session::{
    EnrollmentConnector, EnrollmentEntropy, EnrollmentEntropyFactory, FollowedConversation,
    LocalEnrollment, LocalIssuedEnrollment, NativeSession, SessionArtifact,
    SessionCloseObservation, SessionFailure, SessionFollow, SessionPreparationFailure,
    SessionProduct, SessionRead, SessionRefresh, SessionStatus,
};
