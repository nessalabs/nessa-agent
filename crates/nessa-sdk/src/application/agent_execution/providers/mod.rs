//! Provider adapters implement execution behind the public Agent entry point.
//!
//! ```text
//! Agent -> AgentProvider -> OpenedProviderSession
//!                           |-> ProviderSession -> ProviderSessionBackend
//!                           |-> ExecutionEventStream
//! WorkPermit -> NativeExecutionHandoff -> serialized native dispatch (after preparation)
//! active ExecutionId -> steer -> Injected / PromptRequired
//! ModelChange::between -> validated factory target -> ModelChangeEffect
//!                         -> durable ModelChangeRecord / current configuration
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
//! model capabilities are owned snapshots of the acknowledged current configuration.
//! ApprovalModeChoice is binding-owned catalog data. A live preset change goes
//! through ProviderSessionBackend and is acknowledged only after that binding
//! verifies the effective provider configuration; an uncertain generation is
//! cleaned up before another one is attached.
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
//! HarnessHost is where a binding starts its harness when that is another
//! machine: the binding stays here and speaks its agent protocol over the
//! harness's standard streams, which the host carries (`AgentProvider::on_host`).
//! HarnessHostIdentity borrows that captured owner for live factory comparisons;
//! saved configuration fingerprints do not contain this process-local authority.

mod approval;
mod close;
mod deletion;
mod executable_use;
mod finalized_execution;
mod harness_host;
mod identity;
mod images;
mod input_files;
mod native_hooks;
mod open;
mod operations;
mod ports;
mod reports;
mod session;
mod steering;
pub use approval::{ApprovalMode, ApprovalModeChoice};
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
pub use harness_host::{
    HarnessCleanupFuture, HarnessControl, HarnessHost, HarnessHostIdentity, HarnessLaunch,
    HarnessProcess,
};
pub use identity::ProviderIdentity;
pub use images::{ImageInputRefusal, UserImageError, UserImageFuture, UserImageSource};
pub use input_files::{InputFileFuture, InputFileResolver, PreparedInputFile, UserFileError};
pub use native_hooks::NativeHooksSwitch;
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
    ProviderPermissionDeferralCapability, SessionCloseOwnership, TurnStopCleanup,
};
pub use ports::{
    AgentProvider, CloseOutcome, ExecutionEventStream, NativeExecutionHandoff,
    OpenedProviderSession, ProviderSessionBackend,
};
pub use reports::{
    CleanupFuture, CleanupReport, ExecutionReport, ExecutionReportSource, ObservationFailure,
    ObservationFailureCause, ProviderExecutionFuture, ProviderExecutionReply,
    ProviderObservationFuture, ProviderOperationFailure, ProviderOperationFuture,
    ProviderOperationResult, ProviderOperationStage, ProviderSessionState, ResourceCleanup,
};
pub(crate) use session::validate_configured_input;
pub use session::ProviderSession;

pub use steering::SteeringOutcome;

mod model_change;
pub use model_change::{
    ModelChange, ModelChangeEffect, ModelChangeRecord, ModelChangeTiming, ModelConfiguration,
    SessionConfigurationKind, SessionConfigurationRecord,
};
