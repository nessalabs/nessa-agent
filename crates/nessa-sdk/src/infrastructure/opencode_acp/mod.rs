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
//! the host. It offers its session mode as an ACP config option and its
//! permission policy as configuration; this binding pins both — the mode to
//! `plan` (see `sessions::profile`) and the policy to a read-only one supplied
//! at launch (see `SESSION_POLICY` in `sessions::binding`). The second is the
//! one that bounds the session. `plan` only denies edits.
//!
//! What has been observed of Opencode, and what has not, because the two are
//! not the same and the difference decides how much to trust this module:
//! `initialize` and `session/new` were recorded off 1.18.31 by running it with
//! an empty home. Everything that happens during a model turn — tool calls,
//! permission requests, a mid-turn mode change — was not, because reaching a
//! turn needs OpenCode Zen's host and this was written where the network policy
//! does not allow it. Those shapes come from the ACP specification. Each module
//! that reads one says so, and every session is launched under a policy that
//! denies everything but reading and searching, which is what makes shipping on
//! an assumption tolerable rather than reckless: an assumption about how a tool
//! call is framed costs less when no tool call can act.
pub mod sessions;
pub(crate) mod tools;
