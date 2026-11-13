//! HMAC and HTTPS adapters. The cryptography is the HMAC-SHA256 primitive
//! already used by backup manifests; the webhook framing is its own contract.
mod https;
mod message;
mod outbox;
mod signature;
pub use outbox::{DeliveryAttempt, ResetCommand, ResetReceipt, SqlOutbox};

pub use https::{HttpsWebhook, SystemResolver, MAX_IN_FLIGHT};
pub use message::record_message;
pub use signature::signature;

#[cfg(test)]
#[path = "../../tests/infrastructure/message.rs"]
mod message_tests;

#[cfg(any(test, feature = "test-support"))]
pub use https::isolated_post;

/// The in-memory recipient CA the HTTPS and product delivery tests share.
#[cfg(any(test, feature = "test-support"))]
#[path = "../../tests/infrastructure/fixtures/tls.rs"]
pub mod tls_fixture;
