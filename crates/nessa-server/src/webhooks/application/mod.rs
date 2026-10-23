//! Ports and exact message bytes for an outbound webhook attempt. A MAC is
//! authenticity evidence, not conversation authority.
mod admission;
mod message;
mod ports;

pub use admission::{AdmissionFailure, AdmitDelivery, DeliveryHold, Target};

pub use message::{InvalidMessage, MacKey, Message, MAX_BODY_BYTES};
pub use ports::{
    observation, AddressResolver, PostFailure, PrepareFailure, WebhookHttp, WebhookPost,
    POST_DEADLINE, RESPONSE_BYTES,
};

#[cfg(test)]
#[path = "../../../tests/webhooks/application/message.rs"]
mod message_tests;
