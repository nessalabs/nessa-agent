//! The in-process environment: the agent runs as a child of this gateway, on
//! this machine, exactly as it did before environments had a name. The only
//! [`Environment`] implementation today.
//!
//! ```text
//! open(grant, binding) ──▶ binding, unchanged (the gateway fences it)
//! account(lease)       ──▶ NoProcess (this process never ran that lease)
//! ```
//!
//! Arrows are what each call answers. It enforces no sandbox of its own, so
//! it declares only the harness's default: a lease asking for more is refused
//! by the gateway before this is asked.
use crate::conversation::application::{Environment, EnvironmentDeclaration, EnvironmentFuture};
use nessa_sdk::application::agent_execution::providers::AgentProvider;
use nessa_sdk::domain::agent_execution::leases::{
    EnvironmentRef, LeaseCleanup, LeaseId, LeaseRefusal, LeaseTerms, SandboxProfiles,
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

    fn open(
        &self,
        _grant: &LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, LeaseRefusal> {
        Ok(binding)
    }

    /// A lease this environment is asked about was issued before this
    /// process started — every lease it opens is ended by this process — so
    /// nothing here runs for it.
    fn account<'a>(&'a self, _lease: &'a LeaseId) -> EnvironmentFuture<'a, LeaseCleanup> {
        Box::pin(async { LeaseCleanup::NoProcess })
    }
}

/// The environment composition gives the conversation service.
pub(crate) fn in_process() -> Arc<dyn Environment> {
    Arc::new(InProcessEnvironment)
}
