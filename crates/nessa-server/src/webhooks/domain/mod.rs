//! Pure destination and delivery values. No clock, network or storage reads.
mod value_objects;

pub use value_objects::{
    Delivery, DeliveryError, Destination, InvalidDestination, Observation, Outcome, Phase,
    MAX_ATTEMPTS, MAX_DESTINATION_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/webhooks/domain/delivery.rs"]
mod delivery_tests;

#[cfg(test)]
#[path = "../../../tests/webhooks/domain/destination.rs"]
mod destination_tests;

pub use value_objects::{
    ExpiryObserver, LifecycleChange, LifecycleError, RegistrationLifecycle, RegistrationPhase,
    StopObservation, StopReason,
};

#[cfg(test)]
#[path = "../../../tests/webhooks/domain/registration_lifecycle.rs"]
mod registration_lifecycle_tests;
