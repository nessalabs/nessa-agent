//! A model entity groups the facts for one exact provider/model identity.
//! ModelKey identifies it; description, features, and limits describe it.
//! Keeping those facts together gives the catalog one complete model to manage.
//!
//! ```text
//! Catalog
//!   +-- ModelMetadata
//!         +-- ModelKey
//!         +-- ModelDescription
//!         +-- ModelFeatures
//!         +-- TokenLimits
//! ```
//! This tree shows ownership. The current entity is an immutable snapshot;
//! application callers receive DTO copies rather than access to its fields.

mod model;
pub use model::ModelMetadata;
