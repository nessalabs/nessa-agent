//! Value objects hold immutable values that are safe to use once constructed.
//! Their constructors reject invalid values, so callers share one set of rules.
//! Closely related values share files to keep this feature easy to browse.
//!
//! ```text
//! identity.rs      ModelProvider + ModelKey (provider + model ID)
//! capabilities.rs  Modalities + ModelFeatures + TokenLimits
//! description.rs   ModelDescription (uses common Date)
//!                         |
//!                         v
//!                  model entity / catalog
//! ```
//! The arrow shows where these values are used. They have no independent lifecycle
//! or I/O; callers create new values when the facts change.

mod capabilities;
mod description;
mod identity;
pub use capabilities::{Modalities, ModelFeatures, TokenLimits};
pub use description::ModelDescription;
pub use identity::{ModelKey, ModelProvider};
