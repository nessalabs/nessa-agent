//! Adapters translate outside data and implement application-owned ports.
//! Model metadata and agent execution use separate entry points.
//!
//! ```text
//! model JSON -> model_metadata_json -> application model catalog -> domain
//!
//! host -> claude_acp::sessions -> acp::sessions -> acp::executions
//!      -> codex_acp::sessions  ->      |               |
//!              |                                       |
//!              v                                       v
//!    provider tool translation             application execution controller
//!                                                      |
//!                                                      v
//!                                                domain sessions
//!
//! SessionManager -> session_storage -> leased memory / private snapshot files
//!
//! ACP worker -> tool / permission wire mapping
//!            -> JSON-RPC -> owned process
//! ```
//! Arrows show calls and translation, not ownership shared between layers.
//! Provider profiles supply configuration and tool schemas. Shared ACP owns
//! correlation, deadlines, resume, and cleanup; the domain owns invariants.
//! Composition supplies concrete dependencies and the permission audit sink.

pub mod acp;
pub mod claude_acp;
pub mod codex_acp;
pub mod model_metadata_json;
pub mod session_storage;

pub(crate) mod json_rpc;
pub(crate) mod process;
