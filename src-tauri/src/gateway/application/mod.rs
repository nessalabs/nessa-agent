//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
//! `host_ownership` retains the desktop service-label lease before journal recovery
//! and shared writes. Gateway, reconciliation and stop workers share its lifetime.
//! ClaudePublication serializes live/durable directory changes through ClaudeDirectorySettings;
//! its owned transaction waits for native settlement, then completes any rollback
//! before another directory publication can begin. Tests mirror it in
//! `tests/gateway/application/claude_publication.rs`.
mod host_ownership;
mod ports;
mod service;
pub use crate::gateway::domain::value_objects::ReconciliationHistoryFact;
pub use host_ownership::{DesktopHostClaim, DesktopHostLease};
#[cfg(test)]
pub(crate) use ports::testing;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use ports::GatewayLifecycleRecovery;
#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
pub use ports::GatewayStopProofToken;
pub use ports::{
    ClaudeConfigurationChangeError, ClaudeDirectoryReplacement, ClaudeDirectorySettings,
    ClaudeSettingsPublishError, GatewayError, GatewayHost, GatewayPhysicalResult,
    GatewayReconciliationAttempt, GatewayReconciliationAudit, GatewayReconciliationEffect,
    GatewayReconciliationEffectTiming, GatewayReconciliationIds, GatewayReconciliationIntent,
    GatewayReconciliationIntentDelivery, GatewayReconciliationJournalSession,
    GatewayReconciliationOutcome, GatewayReconciliationOutcomeError, GatewayReconciliationProgress,
    GatewayReconciliationRequest, GatewayStartup, GatewayStartupEvents, GatewayStartupPhase,
    GatewayStopRequest, GatewayStopSession, LoginShellError, LoginShellPath, MonotonicClock,
    ReconciledGateway, StartupStep, SystemMonotonicClock,
};
#[cfg(any(target_os = "macos", test))]
pub use ports::{DeferredStageLaunch, DeferredStageReporter};
pub use service::{Gateway, GatewayRuntimeDependencies};
