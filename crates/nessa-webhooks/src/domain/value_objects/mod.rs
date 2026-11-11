//! Immutable validated destinations and replacement delivery states.
//! Delivery transitions do not mutate stored state or perform network effects.
mod delivery;
mod destination;
mod registration_lifecycle;

pub use delivery::{Delivery, DeliveryError, Observation, Outcome, Phase, MAX_ATTEMPTS};
pub use destination::{Destination, InvalidDestination, MAX_DESTINATION_BYTES};

pub use registration_lifecycle::{
    ExpiryObserver, LifecycleChange, LifecycleError, RegistrationLifecycle, RegistrationPhase,
    StopObservation, StopReason,
};
