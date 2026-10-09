//! The in-process environment: the agent runs as a child of this gateway, on
//! this machine, exactly as it did before environments had a name. Every
//! conversation runs in it unless it was placed on an SSH host, which
//! `ssh_environment` serves.
//!
//! ```text
//! open(lease, grant, binding) ──▶ binding, unchanged (the gateway fences it)
//! hold.end(cause)             ──▶ ByAgentClose (the Agent's close is the evidence)
//! account(lease)              ──▶ NotHeld (nothing this process runs was started under it)
//! ```
//!
//! Arrows are what each call answers. It enforces no sandbox of its own, so
//! it declares only the harness's default: a lease asking for more is refused
//! by the gateway before this is asked.
use crate::conversation::application::{
    Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease, LeaseHold,
    LeaseRelease,
};
use nessa_sdk::application::agent_execution::providers::AgentProvider;
use nessa_sdk::domain::agent_execution::leases::{
    EnvironmentRef, LeaseCleanup, LeaseEndCause, LeaseId, LeaseRefusal, LeaseTerms, SandboxProfiles,
};
use std::sync::Arc;

/// Runs agents in this process.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InProcessEnvironment;

impl Environment for InProcessEnvironment {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: EnvironmentRef::Here,
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }

    fn open<'a>(
        &'a self,
        _lease: &'a LeaseId,
        _grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            Ok(EnvironmentLease {
                provider: binding,
                hold: Arc::new(InProcessHold),
            })
        })
    }

    /// The service asks only about a lease an earlier run of the gateway left
    /// unfinished (see `issue`), so nothing this process runs was started
    /// under it. Whether a harness that run started outlived it is not known
    /// here — a child is killed when its handle drops, which a crash skips —
    /// and `NotHeld` claims no more than that.
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async { Some(LeaseCleanup::NotHeld) })
    }
}

/// In process the lease is held by the Agent alone: nothing can lose it
/// but the gateway, and its close is the cleanup evidence.
struct InProcessHold;

impl LeaseHold for InProcessHold {
    fn lost(&self) -> Option<EnvironmentFuture<'static, ()>> {
        None
    }

    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async { LeaseRelease::ByAgentClose })
    }
}

/// The environment composition gives the conversation service.
pub(crate) fn in_process() -> Arc<dyn Environment> {
    Arc::new(InProcessEnvironment)
}
