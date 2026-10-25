//! HMAC and HTTPS adapters. The cryptography is the HMAC-SHA256 primitive
//! already used by backup manifests; the webhook framing is its own contract.
mod https;
mod message;
mod outbox;
mod runtime;
mod signature;
pub use outbox::SqlOutbox;
pub use runtime::{ConfiguredWebhook, WebhookRuntime};

pub use https::{HttpsWebhook, SystemResolver, MAX_IN_FLIGHT};
pub use message::record_message;
pub use signature::signature;

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/message.rs"]
mod message_tests;

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/admission.rs"]
mod admission_tests;

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/product.rs"]
mod product_tests;
