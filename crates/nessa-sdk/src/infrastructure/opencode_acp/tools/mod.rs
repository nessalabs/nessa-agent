//! Translates Opencode's tool frames into the shared execution vocabulary.
//!
//! ```text
//! ACP session/update -> wire -> ToolCallUpdate
//!                        |
//!                 retained identities -> ToolReviewInput
//! ```
//! Arrows show translation. Opencode is assumed to report tool calls in the
//! protocol's own shape, so the shared translator does the reading; what this
//! module adds is the identity of each call, kept so that a permission request
//! naming only a tool call can still say what is being asked for.
//!
//! Assumed, and said so on purpose. Every frame this module reads — a tool
//! call, its update, a permission request — happens only during a model turn,
//! and no model turn could be run where this was written: reaching one needs
//! OpenCode Zen's host, which the build environment's network policy does not
//! allow. What was recorded off Opencode 1.18.31 is `initialize` and
//! `session/new`, neither of which is read here. So these shapes come from the
//! ACP specification, and the first real turn is what will confirm or correct
//! them. Two places that would have to change if it corrects them: this module
//! assumes a permission request carries its arguments in `rawInput` and refuses
//! one that does not, where Codex needed a `_meta` fallback for exactly that
//! case; and it assumes a tool call declares an ACP `kind`, which is what a
//! permission request is named by.
pub(super) mod wire;
