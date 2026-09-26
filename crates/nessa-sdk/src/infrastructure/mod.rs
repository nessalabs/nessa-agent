//! Adapters translate outside data and implement application-owned ports.
//! Model metadata and agent execution use separate entry points.
//!
//! ```text
//! model JSON -> model_metadata_json -> application model catalog -> domain
//!
//!         claude_acp::sessions
//! host -> or codex_acp::sessions -> acp::sessions -> acp::executions
//!         or opencode_acp::sessions     |                 |
//!              |                        |                 |
//!              v                        v                 v
//!    provider tool translation     shared runtime   application execution
//!                                                       controller
//!                                                          |
//!                                                          v
//!                                                   domain sessions
//!
//! SessionManager -> session_storage -> leased memory / private snapshot files
//!
//! ACP worker -> tool / permission wire mapping
//!            -> JSON-RPC -> owned process
//! ```
//! Arrows show calls and translation, not ownership shared between layers. The
//! vendor `sessions` modules are alternatives, not a chain: a host reaches one
//! of them, and each hands the same shared ACP runtime a profile.
//! Provider profiles supply configuration and tool schemas. Shared ACP owns
//! correlation, deadlines, resume, and cleanup; every deadline is a moment on
//! the injected `clock`. The domain owns invariants.
//! Composition supplies concrete dependencies and the permission audit sink.

pub mod acp;
pub mod claude_acp;
pub mod clock;
pub mod codex_acp;
pub mod model_metadata_json;
pub mod opencode_acp;
pub mod session_storage;

pub(crate) mod json_rpc;
pub(crate) mod process;
