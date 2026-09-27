//! The catalog aggregate protects rules that span multiple model entries.
//! It requires at least one model, unique provider/model keys, and a verification
//! date with day precision. A valid model alone cannot guarantee these rules.
//!
//! ```text
//! application import
//!         |
//!         v
//! Catalog (checks the whole collection)
//!   +-- ModelMetadata
//!   +-- ModelMetadata
//!         |
//!         v
//! application list / select --> DTO copies
//! ```
//! Arrows show the flow from construction to queries. The catalog owns its models
//! and exposes read-only access. It does not load files or run providers.

mod catalog;
pub use catalog::Catalog;
