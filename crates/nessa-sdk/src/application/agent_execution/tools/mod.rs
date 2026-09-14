//! Opaque tool input preserves the provider arguments shown during review.
//! Domain observations describe tool state; provider adapters validate input schemas.
//!
//! ```text
//! provider adapter --> ToolReviewInput --> permission review
//! ```
//!
//! Arrows show data flow. This module does not execute tools or grant access.

mod review_input;
pub use review_input::ToolReviewInput;
