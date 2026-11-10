//! What an agent's environment tools are allowed and how long its artifact
//! calls wait: who may run which commands where (`run_command`), and the
//! waits of `artifacts_publish` and `artifacts_fetch`. Where a conversation
//! runs is [`super::Environments`]'s; these are the tools' own.
use super::CommandPolicyEvaluator;
use std::{sync::Arc, time::Duration};

/// How long an `artifacts_publish` call waits, from its ask, for its file
/// to be staged, read, checked and kept before it answers that it does not
/// know (`timed_out`). It is longer than one read's own bound (row
/// `artifact.publish_deadline`), but the collector takes a lease's files one
/// at a time, so a file queued behind others may still be read after its
/// call stopped waiting; it is then kept for the call's person all the same.
/// Row `artifact.publish_call_wait` in `docs/limits.md`.
pub const PUBLISH_CALL_WAIT: Duration = Duration::from_secs(16 * 60);

/// How long an `artifacts_fetch` call waits, from its ask, for its file to
/// be written to the environment, checked and placed before it answers that
/// it does not know (`timed_out`). A file is written range by range from the
/// conversation's hold, each range waiting at most its read's busy wait.
/// Stopped before every byte was written, the environment is told so and
/// places nothing. Row `artifact.fetch_call_wait` in `docs/limits.md`.
pub const FETCH_CALL_WAIT: Duration = Duration::from_secs(16 * 60);

/// The environment tools' policy and waits, handed to the conversation
/// service with its other dependencies.
#[derive(Clone)]
pub struct EnvironmentToolSettings {
    /// Who may run which commands on which hosts, asked of the command
    /// policy; `None` when this gateway grants none, and the environment
    /// tools are absent.
    pub(crate) commands: Option<Arc<dyn CommandPolicyEvaluator>>,
    /// How long an `artifacts_publish` call waits for its file to be kept.
    pub(crate) publish_wait: Duration,
    /// How long an `artifacts_fetch` call waits for its file to be placed.
    pub(crate) fetch_wait: Duration,
}

impl Default for EnvironmentToolSettings {
    /// No command policy, and the artifact calls' own waits.
    fn default() -> Self {
        Self {
            commands: None,
            publish_wait: PUBLISH_CALL_WAIT,
            fetch_wait: FETCH_CALL_WAIT,
        }
    }
}

impl EnvironmentToolSettings {
    /// These tools, with commands granted as `policy` answers.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_commands(policy: Arc<dyn CommandPolicyEvaluator>) -> Self {
        Self {
            commands: Some(policy),
            ..Self::default()
        }
    }
}
