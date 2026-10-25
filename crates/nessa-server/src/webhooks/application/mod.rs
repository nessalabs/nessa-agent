//! Ports and exact message bytes for an outbound webhook attempt. A MAC is
//! authenticity evidence, not conversation authority.
mod admission;
mod message;
mod outbox;
mod ports;
mod registration;
mod registration_stop;
mod share;
mod source;
pub use share::ShareWebhookConversation;
pub use source::{ReadWebhookRecords, WebhookReadFailure, WebhookRecords};

pub(crate) use admission::DispatchAdmission;
pub use admission::{AdmissionFailure, AdmitDelivery, DeliveryHold, Target};
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

pub use message::{InvalidMessage, MacKey, Message, MAX_BODY_BYTES};
pub use ports::{
    observation, AddressResolver, PostFailure, PrepareFailure, WebhookHttp, WebhookPost,
    POST_DEADLINE, RESPONSE_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/webhooks/application/message.rs"]
mod message_tests;

pub use crate::webhooks::domain::{ExpiryObserver, StopReason};
