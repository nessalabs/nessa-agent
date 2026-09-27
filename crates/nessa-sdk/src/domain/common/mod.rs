//! Shared domain primitives used by feature models.
//! Keep only concepts whose meaning and rules are independent of any feature.
//!
//! ```text
//! model_metadata --> common --> pure parsing libraries
//! future feature --> common
//!                    (chrono, url; no I/O)
//! ```
//! Arrows mean dependencies. Common never imports a feature or an outer layer.

pub mod value_objects;
