//! What a runtime is, and whether it has been prepared.
//!
//! There is no entity or aggregate here, and inventing one would be dishonest:
//! a warm-up has no identity of its own and protects no mutable state. It has
//! either been done for a given runtime or it has not, and the runtime is named
//! by a value.
pub mod value_objects;
pub use value_objects::{RuntimeFingerprint, RuntimeFingerprintError, WarmUpState};
