//! Translates Opencode's tool frames into the shared execution vocabulary.
//!
//! ```text
//! ACP session/update -> wire -> ToolCallUpdate
//!                        |
//!                 retained identities -> ToolReviewInput
//! ```
//! Arrows show translation. Opencode reports tool calls in the protocol's own
//! shape, so the shared translator does the reading; what this module adds is
//! the identity of each call, kept so that a permission request naming only a
//! tool call can still say what is being asked for.
pub(super) mod wire;
