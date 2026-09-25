//! Provider adapters implement execution behind the public Agent entry point.
//!
//! ```text
//! Agent -> AgentProvider -> OpenedProviderSession
//!                           |-> ProviderSession -> ProviderSessionBackend
//!                           |-> ExecutionEventStream
//! active ExecutionId -> steer -> Injected / PromptRequired
//! SessionCloseRequest -> backend close -> CleanupReport (resources + audit)
//! failed open -> ProviderOpenError -> ProviderCleanup (when resources remain)
//! executable snapshot -> per-generation use guard -> release after confirmed cleanup
//! finalized adapter facts -> validated recipe -> ExecutionReport -> storage
//! ```
//! Arrows show calls and returned handles. These ports describe a complete
//! invocation runtime; ACP need not expose individual model/tool steps.
//! ProviderIdentity validates and compacts exact resume components before any
//! adapter can return them; snapshot decoding uses the same constructor.
//! Shutdown requests preserve the initiating cause; returned outcomes separately
//! describe whether resource cleanup was confirmed. Operation failures carry explicit
//! provider session status; execution reports distinguish rejection from observed settlement.
//! Diagnostic errors never determine admission or resource ownership.
//! ProviderOperationCapabilities flows from the live backend into ProviderSession,
//! which resolves the private effective OperationCapabilities read by Agent;
//! immutable model capabilities remain a separate admission contract.
//! Admission reads one negotiated fact from it: an image message is refused when
//! the agent is known not to take images, and admitted while that is not yet known.
//! A backend's `validate_input` adds what it could never deliver, such as a
//! message too large for one frame. Every refusal of a message's images is an
//! ImageInputRefusal, the same value at admission and at dispatch.
//! UserImageSource is how an adapter turns a message's image references into
//! bytes just before dispatch, off the task that owns the provider connection;
//! requests, queues, and snapshots only ever hold references.
//! ProviderSessionDeleter is how one agent's binding deletes that agent's own
//! record of a session nothing runs any more, over a connection of its own
//! that never resumes the session; each binding says what a successful delete
//! means for its agent (ProviderSessionDeletion).

mod close;
mod deletion;
mod executable_use;
mod finalized_execution;
mod identity;
mod images;
mod open;
mod operations;
mod ports;
mod reports;
mod session;
mod steering;
pub use close::SessionCloseRequest;
pub use deletion::{
    ProviderSessionDeleter, ProviderSessionDeletion, ProviderSessionDeletionFuture,
};
pub use executable_use::{
    ExecutableUse, ExecutableUseAdmissionFailure, ExecutableUseAdmissionOwner, ExecutableUseError,
    ExecutableUseGuard, ExecutableUseSnapshot,
};
pub(crate) use finalized_execution::{
    FinalizedExecutionProjection, FinalizedExecutionSource, FinalizedFailureComponent,
};
pub use identity::ProviderIdentity;
pub use images::{ImageInputRefusal, UserImageError, UserImageFuture, UserImageSource};
pub(crate) use open::{FailedOpenCauseSource, FailedOpenCleanup};
pub use open::{
    ProviderCleanup, ProviderOpenControl, ProviderOpenError, ProviderOpenFuture,
    ProviderOpenRequest,
};
pub use operations::{
    CompactionReportingCapability, ElicitationForwardingCapability, IncomingElicitationCapability,
    ModelSwitchReportingCapability, NativeHookSuppressionCapability, OperationCapabilities,
    PermissionDeferralCapability, PermissionDenialCapability, PolicyCloseSessionCapability,
    PolicyEndTurnCapability, PreToolPolicyCapability, ProviderCompactionReportingCapability,
    ProviderModelSwitchReportingCapability, ProviderOperationCapabilities,
    ProviderPermissionDeferralCapability,
};
pub use ports::{
    AgentProvider, CloseOutcome, ExecutionEventStream, OpenedProviderSession,
    ProviderSessionBackend,
};
pub use reports::{
    CleanupFuture, CleanupReport, ExecutionReport, ExecutionReportSource, ObservationFailure,
    ObservationFailureCause, ProviderExecutionFuture, ProviderExecutionReply,
    ProviderObservationFuture, ProviderOperationFailure, ProviderOperationFuture,
    ProviderOperationResult, ProviderSessionState, ResourceCleanup,
};
pub(crate) use session::validate_configured_input;
pub use session::ProviderSession;

pub use steering::SteeringOutcome;
