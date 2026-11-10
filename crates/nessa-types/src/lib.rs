//! Names and bounds that more than one Nessa crate must agree on.
//!
//! A value lives here when the SDK and the wire protocol both name it, so
//! neither has to depend on the other to agree. Each one is still owned by
//! the rule that uses it: the SDK checks a published file against
//! [`MAX_PUBLISHED_FILE_BYTES`], the protocol tells an environment the same
//! number, and both read it from here.
//!
//! Admission rule: no IO, no runtime, no dependencies. A type with behavior
//! beyond naming a value stays with the crate whose rule it is.
#![deny(missing_docs)]

/// The provider name each agent binding records in a session's history, as
/// its provider identity.
pub mod providers {
    /// Claude's binding (`claude-agent-acp`).
    pub const CLAUDE_ACP: &str = "claude-acp";
    /// Codex's binding (`codex-acp`).
    pub const CODEX_ACP: &str = "codex-acp";
    /// OpenCode's binding (`opencode acp`).
    pub const OPENCODE_ACP: &str = "opencode-acp";
}

/// Bounds on what one lease's environment publishes and fetches.
pub mod leases {
    /// Most bytes one published file may have: the most one attachment may
    /// hold, since the conversation keeps the file as one.
    pub const MAX_PUBLISHED_FILE_BYTES: u64 = 64 * 1024 * 1024;
    /// Longest artifact name, in bytes: a file name's bound.
    pub const MAX_ARTIFACT_NAME_BYTES: usize = 255;
    /// Longest path on an environment, in bytes: the most an environment
    /// reads or answers.
    pub const MAX_ENVIRONMENT_PATH_BYTES: usize = 1024;
    /// Most fetches one lease has asked and not yet ended.
    pub const MAX_ASKED_FETCHES: usize = 4;

    /// What the environment must enforce around the agent's commands. A
    /// profile is declared by whoever can set it up and refused where it
    /// cannot be, never silently weakened ("Sandboxes, honestly").
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum SandboxProfile {
        /// Whatever the harness does by default, configured by nothing Nessa
        /// sets. It encloses only what the harness itself encloses.
        HarnessDefault,
        /// Nothing encloses the work: it runs as the environment's account.
        /// What a command lease's environment holds, since a command has no
        /// harness to sandbox it.
        None,
    }
}
