//! The product protocol served at `/session`, as both ends of the socket see it.
//!
//! ```text
//! catalogue_read ─┐
//!                 ├─▶ passive_read ─▶ generated
//! record_read ────┘
//! handshake ─────────────────────────▶ generated
//! ```
//! Arrows are imports.
//!
//! - `generated` holds the DTOs, method and event names, and bounds generated
//!   from `protocol/product/v1.json` by `scripts/generate-product-protocol.mjs`.
//! - `handshake` holds the version-overlap and refusal-to-close rules both
//!   ends of the handshake apply.
//! - `passive_read` owns the shared passive read transport conversion and the
//!   capped response encoder; `record_read` and `catalogue_read` are the two
//!   codecs that consume it, each holding both directions so a gateway answer
//!   and a client's reading of it cannot drift.
pub mod catalogue_read;
pub mod generated;
pub mod handshake;
pub mod passive_read;
pub mod record_read;
