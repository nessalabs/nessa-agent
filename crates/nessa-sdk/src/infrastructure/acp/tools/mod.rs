//! Translates sparse ACP tool descriptions into immutable domain updates.
//!
//! ```text
//! ACP tool JSON -> wire -> domain ToolCallUpdate
//! ```
//! Arrows show translation. Identity and lifecycle decisions stay in the domain.
//! Structurally tagged result variants without a Nessa representation become a
//! bounded visible placeholder; malformed known variants remain protocol errors.
//! `with_forwarded` appends to an MCP call's terminal update the structured
//! result its stand-in forwarded (`infrastructure::mcp::ForwardedResults`),
//! which Claude's harness reports only as text.
//! Parser regressions live in `tests/infrastructure/acp/tools/wire.rs` and are
//! registered privately by the wire adapter.
pub(crate) mod wire;
