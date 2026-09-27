//! Validates the configured Claude file-tool schemas and retains exact input.
//!
//! ```text
//! Claude tool JSON -> wire -> ToolCallUpdate + ToolReviewInput
//! ```
//! The arrow shows translation. These descriptions confer no access authority;
//! shared ACP permission handling delivers the caller's domain-validated choice.
//! Schema and sparse-update regressions live under
//! `tests/infrastructure/claude_acp/tools/wire.rs`, registered privately by wire.
pub(in crate::infrastructure::claude_acp) mod wire;
