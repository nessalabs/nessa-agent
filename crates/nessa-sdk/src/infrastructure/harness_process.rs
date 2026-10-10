//! A harness started on this machine for someone else's binding: the
//! process tree an environment supervises when the binding that speaks to it
//! runs elsewhere (`HarnessHost` on that side).
//!
//! ```text
//! spawn(command) ──▶ SupervisedHarness ──cleanup(grace, kill)──▶ CloseOutcome
//!        └──▶ stdin / stdout, carried to the binding by the environment
//! ```
//!
//! Arrows are calls and what they hand back. It is the same process scope a
//! binding gives its own child — a process group of its own, stderr drained
//! and never kept, end-of-input first, then a signal, then a forced kill, each
//! within its budget — so cleanup evidence means the same wherever the harness
//! ran.
#![deny(missing_docs)]

use super::process::{ProcessInput, ProcessOutput, ProcessScope};
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessProcess},
};
use std::time::Duration;
use tokio::process::{ChildStdin, ChildStdout, Command};

/// One harness process tree this machine runs and is answerable for.
///
/// Dropping it without a confirmed [`Self::cleanup`] kills its process group.
pub struct SupervisedHarness {
    scope: ProcessScope,
}

impl SupervisedHarness {
    /// Start `command` in a process group of its own, with piped standard
    /// streams, and hand back its input and output.
    ///
    /// # Errors
    /// [`AgentError::Transport`] when the process could not be started;
    /// nothing runs then.
    pub fn spawn(command: Command) -> Result<(Self, ChildStdin, ChildStdout), AgentError> {
        Self::spawned(command)
    }

    /// [`Self::spawn`], as the [`HarnessProcess`] a host hands a binding.
    ///
    /// # Errors
    /// As [`Self::spawn`].
    pub fn start(command: Command) -> Result<HarnessProcess, AgentError> {
        let (harness, input, output) = Self::spawned(command)?;
        Ok(HarnessProcess {
            input: Box::new(input),
            output: Box::new(output),
            control: Box::new(harness),
        })
    }

    fn spawned(command: Command) -> Result<(Self, ChildStdin, ChildStdout), AgentError> {
        let mut scope = ProcessScope::spawn(command)?;
        let input = match scope.stdin.take() {
            Some(ProcessInput::Local(input)) => input,
            _ => unreachable!("a spawned scope has its child's piped stdin"),
        };
        let output = match scope.stdout.take() {
            Some(ProcessOutput::Local(output)) => output,
            _ => unreachable!("a spawned scope has its child's piped stdout"),
        };
        Ok((Self { scope }, input, output))
    }

    /// Release the process tree: `grace` to leave by itself once its input
    /// is closed (the caller drops the input it was given first), then a
    /// signal, then a forced kill, each waited on for at most `kill_timeout`.
    /// Asking again after a confirmation answers the same confirmation.
    ///
    /// # Errors
    /// [`AgentError::CleanupUncertain`] when the tree is not known to be gone.
    pub async fn cleanup(
        &mut self,
        grace: Duration,
        kill_timeout: Duration,
    ) -> Result<CloseOutcome, AgentError> {
        self.scope.cleanup(grace, kill_timeout).await
    }
}

impl HarnessControl for SupervisedHarness {
    fn cleanup(&mut self, grace: Duration, kill_timeout: Duration) -> HarnessCleanupFuture<'_> {
        Box::pin(SupervisedHarness::cleanup(self, grace, kill_timeout))
    }
}
