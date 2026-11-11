//! The serial runtime that reads committed records through the conversation
//! context and hands them to the `nessa_webhooks` outbox and HTTPS sender.
mod runtime;
pub(crate) use runtime::DeliveryDependencies;
pub use runtime::{ConfiguredWebhook, WebhookRuntime};

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/admission.rs"]
mod admission_tests;

#[cfg(test)]
#[path = "../../../tests/webhooks/infrastructure/product.rs"]
mod product_tests;
