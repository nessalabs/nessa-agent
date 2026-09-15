//! Provider adapters implement execution behind the public Agent entry point.
//!
//! ```text
//! Agent -> AgentProvider -> OpenedProviderSession
//!                           |-> ProviderSession -> ProviderSessionBackend
//!                           |-> ExecutionEventStream
//! active ExecutionId -> steer -> Injected / PromptRequired
//! SessionCloseRequest -> backend close -> CleanupReport (resources + audit)
//! failed open -> ProviderOpenError -> ProviderCleanup (when resources remain)
//! ```
//! Arrows show calls and returned handles. These ports describe a complete
//! invocation runtime; ACP need not expose individual model/tool steps.
//! ProviderIdentity validates and compacts exact resume components before any
//! adapter can return them; snapshot decoding uses the same constructor.
//! Shutdown requests preserve the initiating cause; returned outcomes separately
//! describe whether resource cleanup was confirmed. Operation failures carry explicit
//! provider session status; execution reports distinguish rejection from observed settlement.
//! Diagnostic errors never determine admission or resource ownership.
//! OperationCapabilities flows from the live backend through ProviderSession to
//! Agent; immutable model capabilities remain a separate admission contract.

mod close;
mod identity;
mod open;
mod operations;
mod ports;
mod reports;
mod session;
mod steering;
pub use close::SessionCloseRequest;
pub use identity::ProviderIdentity;
pub use open::{ProviderCleanup, ProviderOpenError, ProviderOpenFuture};
pub use operations::OperationCapabilities;
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
pub use session::ProviderSession;

pub use steering::SteeringOutcome;
