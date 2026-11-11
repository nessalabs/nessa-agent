//! Ports and exact message bytes for an outbound webhook attempt. A MAC is
//! authenticity evidence, not conversation authority.
mod message;
mod outbox;
mod ports;
mod registration;
mod registration_stop;
mod target;

pub use outbox::{
    DeliveryCause, Outbox, OutboxError, PendingDelivery, QueueInitiator, QueuedRecord,
    Registration, MAX_CURSORS, MAX_OUTBOX_WORKERS, MAX_REGISTRATIONS, MAX_SOURCE_RESETS,
    PENDING_LIMIT, RETAINED_LIMIT, SOURCE_RESET_BYTES,
};
pub use registration::{
    RegisterWebhook, RegisteredWebhook, RegistrationConfig, RegistrationFailure,
    RegistrationIntent, RegistrationIntents, RegistrationLookup, UninstalledCancellation,
};
pub(crate) use registration_stop::StopFact;
pub use registration_stop::{
    ObserveRegistrationStop, RegistrationStop, RegistrationStops, StopFailure,
    StopObservationResult,
};
pub use target::Target;

pub use message::{InvalidMessage, MacKey, Message, MAX_BODY_BYTES};
pub use ports::{
    observation, AddressResolver, PostFailure, PrepareFailure, WebhookHttp, WebhookPost,
    POST_DEADLINE, RESPONSE_BYTES,
};

#[cfg(test)]
#[path = "../../tests/application/message.rs"]
mod message_tests;

pub use crate::domain::{ExpiryObserver, StopReason};
