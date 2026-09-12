//! Immutable configuration and requirements used to validate new input.
//! The snapshot narrows model metadata; it never changes the catalog.
mod capabilities;
pub use capabilities::{
    BindingRestrictions, CapabilityError, CapabilityRequirement, EffectiveCapabilities, Modality,
};
