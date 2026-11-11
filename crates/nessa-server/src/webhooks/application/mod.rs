//! Admission of a webhook delivery: the current credential and principal-grant
//! facts it consumes, the principal share it reads as, and the bounded
//! canonical record source. Each meets the conversation context, so it stays
//! in the server; the outbox, registration and message ports are
//! `nessa_webhooks`.
mod admission;
mod share;
mod source;
pub use share::ShareWebhookConversation;
pub use source::{ReadWebhookRecords, WebhookReadFailure, WebhookRecords};

pub(crate) use admission::{validate_target_snapshot, DispatchAdmission};
pub use admission::{AdmissionFailure, AdmitDelivery, DeliveryHold};
