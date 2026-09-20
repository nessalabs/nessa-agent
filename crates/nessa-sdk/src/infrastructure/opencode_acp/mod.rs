//! Opencode provider configuration and tool translation for the shared ACP runtime.
//!
//! ```text
//! sessions::OpencodeAcpProvider -> Opencode profile -> shared ACP sessions
//!                                       |
//!                                 tools wire mapping
//! ```
//! Arrows show construction and calls. Shared ACP owns execution and permission
//! transport; this provider module supplies the configuration Opencode opens a
//! session from, the checks that confirm it took it, and the translation of the
//! tool frames it reports.
//!
//! Opencode is the agent Nessa offers to somebody who has neither Claude Code
//! nor Codex on their machine, so it is the one whose first run has to work with
//! nothing set up. It does: `opencode acp` opens a session against OpenCode Zen
//! with no account and no key. The models that reaches are free ones the gateway
//! rotates, which is why this binding reads back the model it asked for rather
//! than assuming the one it named is still offered.
//!
//! Like Codex, Opencode brings its own tools rather than taking a tool set from
//! the host, and offers its approval policy as an ACP config option. This
//! binding pins the least permissive one it has — see `sessions::profile`.
//!
//! What has been observed of Opencode, and what has not, because the two are
//! not the same and the difference decides how much to trust this module:
//! `initialize` and `session/new` were recorded off 1.18.31 by running it with
//! an empty home. Everything that happens during a model turn — tool calls,
//! permission requests, a mid-turn mode change — was not, because reaching a
//! turn needs OpenCode Zen's host and this was written where the network policy
//! does not allow it. Those shapes come from the ACP specification. Each module
//! that reads one says so, and `MODE` keeps every session in the mode that runs
//! nothing, which is what makes shipping on an assumption tolerable rather than
//! reckless.
pub mod sessions;
pub(crate) mod tools;
