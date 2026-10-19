//! Where a binding starts its harness when that is not this machine.
//!
//! ```text
//! binding.on_host(host) ──▶ same binding, launching through `host`
//!     open ──▶ HarnessHost::start(HarnessLaunch) ──▶ HarnessProcess
//!                                                    ├─ input  (harness stdin)
//!                                                    ├─ output (harness stdout)
//!                                                    └─ control.cleanup ──▶ CloseOutcome
//! ```
//!
//! Arrows are calls and what they hand back. The binding stays where the
//! conversation is kept and speaks its agent protocol over `input` and
//! `output` exactly as it does to a child process of its own; the host runs
//! the harness process next to its files, supervises its process tree, and
//! reports what releasing it took. A host chooses the executable, its
//! arguments, the account's own variables and the credentials it signs in
//! with: [`HarnessLaunch`] carries only what the binding itself sets for one
//! launch, never a credential and never a path on this machine.
#![deny(missing_docs)]

use super::CloseOutcome;
use crate::application::agent_execution::agents::AgentError;
use std::{
    collections::BTreeMap, ffi::OsString, future::Future, path::Path, pin::Pin, sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};

/// What a binding sets for one launch of its harness on a host: the
/// variables that select its model, its output budget and its preset. The
/// host adds its own executable, arguments, account variables and
/// credentials; none of those come from here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HarnessLaunch {
    /// Binding-owned variables, in addition to the host's own.
    pub environment: BTreeMap<OsString, OsString>,
}

/// The answer to a [`HarnessCleanupFuture`]: what releasing the harness's
/// process tree took, or why it is not known to be released.
pub type HarnessCleanupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CloseOutcome, AgentError>> + Send + 'a>>;

/// How the binding stops a harness a host started.
///
/// Dropping it without a confirmed [`Self::cleanup`] must still ask the host
/// to stop the process tree, by force: a dropped handle is never permission
/// for the harness to keep running.
pub trait HarnessControl: Send {
    /// Release the harness: its input is already closed, so it has `grace` to
    /// leave by itself, then the host stops its process tree, waiting at most
    /// `kill_timeout` for each step. `Ok` is the host's confirmation that the
    /// tree is gone, `forced` saying whether a signal was needed;
    /// [`AgentError::CleanupUncertain`] is anything less, including a host
    /// that could not be asked or did not answer.
    fn cleanup(&mut self, grace: Duration, kill_timeout: Duration) -> HarnessCleanupFuture<'_>;
}

/// One harness a host started, as the binding talks to it.
pub struct HarnessProcess {
    /// The harness's standard input. Shutting it down is the end-of-input the
    /// agent protocol asks a harness to leave on.
    pub input: Box<dyn AsyncWrite + Send + Unpin>,
    /// The harness's standard output. End of stream is the harness gone or
    /// the host no longer reachable; either way nothing more arrives.
    pub output: Box<dyn AsyncRead + Send + Unpin>,
    /// How it is stopped.
    pub control: Box<dyn HarnessControl>,
}

/// A machine other than this one that a binding can start its harness on.
pub trait HarnessHost: Send + Sync {
    /// The directory on the host the harness works in: its working
    /// directory and the agent protocol's session workspace. Absolute.
    fn workspace(&self) -> &Path;
    /// Start the harness for `launch`. Answers at once, without waiting for
    /// the host: a harness the host then fails to start is an output that
    /// ends before the agent protocol's first answer, which the binding
    /// already reports as a failed start, and a cleanup that confirms
    /// nothing ran.
    ///
    /// # Errors
    /// The typed reason nothing could be asked of the host at all, such as
    /// [`AgentError::Closed`] once its connection is gone.
    fn start(&self, launch: HarnessLaunch) -> Result<HarnessProcess, AgentError>;
}

/// Borrowed identity of the host a provider factory uses for its harness.
/// Local identities compare equal; hosted identities compare the captured Arc
/// owner, independently of workspace spelling or launch configuration. This view
/// exposes no host handle and carries no persistent identity across restarts.
///
/// ```
/// use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
/// assert!(HarnessHostIdentity::local() == HarnessHostIdentity::local());
/// ```
#[derive(Clone, Copy)]
pub struct HarnessHostIdentity<'host> {
    host: Option<&'host Arc<dyn HarnessHost>>,
}
impl<'host> HarnessHostIdentity<'host> {
    /// Assert that this factory starts its harness in this process's local environment.
    pub const fn local() -> Self {
        Self { host: None }
    }
    /// Borrow the factory's captured host owner without granting start authority.
    /// The view cannot outlive the borrowed Arc and does not acquire another owner.
    pub fn hosted(host: &'host Arc<dyn HarnessHost>) -> Self {
        Self { host: Some(host) }
    }
}
impl PartialEq for HarnessHostIdentity<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self.host, other.host) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}
impl Eq for HarnessHostIdentity<'_> {}
