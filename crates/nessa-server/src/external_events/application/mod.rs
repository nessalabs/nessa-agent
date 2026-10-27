//! Event admission reuses current auth and the durable conversation command owner.
mod admission;
pub use admission::{Admission, AdmissionFailure, EventBinding};
