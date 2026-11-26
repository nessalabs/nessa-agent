//! Sinks for real backends.
mod otel;

pub use otel::{OtelSettings, OtelSink, OtelUnavailable};
