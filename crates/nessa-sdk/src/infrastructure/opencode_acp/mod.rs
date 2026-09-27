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
//! This SDK module implements the provider mechanism; it does not choose Nessa's
//! supported model or credential policy. Its caller supplies model metadata to
//! the provider and the credential environment in
//! [`crate::infrastructure::acp::sessions::AcpConfig`]. The session verifies
//! that Opencode actually offers the requested model. Private HOME and XDG
//! roots prevent the binding from discovering a separate account through caller
//! configuration or `auth.json`.
//!
//! Nessa's packaged composition supplies a saved stage-scoped API key and starts
//! on the metered MiniMax M3 Zen model. Its standalone composition instead uses
//! the explicit runtime policy and `OPENCODE_API_KEY` captured when composition
//! starts. Upstream may offer other models, including free catalogue entries;
//! this module neither promises their eligibility nor forbids a caller from
//! selecting one it has independently admitted.
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
//! an empty home. That observation establishes protocol shape, not account or
//! model-call eligibility. Everything that happens during a model turn — tool
//! calls, permission requests, a mid-turn mode change — was not, because
//! reaching a turn needs OpenCode Zen's host and this was written where the
//! network policy does not allow it. Those shapes come from the ACP
//! specification. Each module that reads one says so, and every session is
//! launched under a policy that denies everything but reading and searching.
//! That makes shipping on an assumption tolerable rather than reckless: an
//! assumption about how a tool call is framed costs less when no tool call can
//! act.
pub mod sessions;
pub(crate) mod tools;
