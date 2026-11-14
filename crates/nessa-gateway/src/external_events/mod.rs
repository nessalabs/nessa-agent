//! Inbound event admission into an existing conversation.
//!
//! ```text
//! HTTP signature -> bounded Event -> current Auth/Cedar -> durable submit -> SDK
//! ```
//! Arrows are calls. The SDK owns deduplication, execution, permissions and stop.
//! Composition must mount the router explicitly; production does not mount it.
#![deny(missing_docs)]

pub mod application;
pub mod domain;
pub mod entrypoint;

#[cfg(test)]
#[path = "../../tests/external_events/ingress.rs"]
mod tests;
