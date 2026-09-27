//! Model metadata describes which model an entry refers to and what it supports.
//! This feature keeps those facts valid before the application exposes them.
//!
//! ```text
//! application import --> Catalog --> ModelMetadata --> value objects
//!                        aggregate    entity           identity
//!                                                     capabilities
//!                                                     description
//! ```
//! The catalog checks the collection; each model holds validated values.
//! These are published model facts. A harness's configured context window is a
//! separate execution setting, and provider execution is not implemented here.

pub mod aggregates;
pub mod entities;
mod error;
pub mod value_objects;
pub use error::MetadataError;
