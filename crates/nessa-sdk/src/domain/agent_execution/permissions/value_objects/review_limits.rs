//! How much one review may hold, and how many may be open at once.
//!
//! These are the one copy of each bound. A live request past one is refused
//! when it arrives, as a declined review the agent is told about, so no
//! surface is ever left holding a review it cannot show. The product schema
//! publishes the same numbers, and its generator refuses to run when they
//! differ (`scripts/generate-product-protocol.mjs`).

/// Most reviews one execution may have open at once. A surface offers every
/// one of them.
pub const MAX_OPEN_REVIEWS: usize = 128;
/// Most choices one review may offer.
pub const MAX_REVIEW_OPTIONS: usize = 64;
/// Longest label one choice may have, in UTF-8 bytes.
pub const MAX_REVIEW_OPTION_LABEL_BYTES: usize = 2048;
/// Longest tool name a review may carry, in UTF-8 bytes.
pub const MAX_REVIEW_TOOL_NAME_BYTES: usize = 256;
