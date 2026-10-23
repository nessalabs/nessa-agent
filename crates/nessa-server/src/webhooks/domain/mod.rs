//! Pure destination and delivery values. No clock, network or storage reads.
mod value_objects;

pub use value_objects::{
    Delivery, DeliveryError, Destination, InvalidDestination, Observation, Outcome, Phase,
    MAX_ATTEMPTS, MAX_DESTINATION_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/webhooks/domain/delivery.rs"]
mod delivery_tests;
