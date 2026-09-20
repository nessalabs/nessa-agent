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
pub mod sessions;
pub(crate) mod tools;
