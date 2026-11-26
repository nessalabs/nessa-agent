//! Emit telemetry once; send it to whichever backends are turned on.
//!
//! This crate knows nothing of the application that uses it. An [`Event`] is
//! a name, a [`Shape`] and attributes, and the set of events is open: the
//! application names its own. [`Telemetry`] is the one emit point, and hands
//! each event to every [`Sink`] that wants its shape, off the caller's
//! thread. A sink is one backend: [`OtelSink`] (feature `otel`) sends spans
//! to an OpenTelemetry collector, and another backend is another sink.
//!
//! ```text
//! domain/          Event, Shape, SpanRef, Value, Attributes  — what an event says
//! application/     Sink (one backend), Telemetry (the dispatcher)
//! infrastructure/  OtelSink                                    — OTLP/HTTP
//! ```
//!
//! Telemetry may be lost (a full queue drops, a failing sink loses an event);
//! a record that must not be lost belongs in an audit store instead.
#![deny(missing_docs)]

mod application;
mod domain;
#[cfg(feature = "otel")]
mod infrastructure;

pub use application::{Sink, Telemetry, DEFAULT_CAPACITY};
pub use domain::{
    Attributes, Event, MeasureKind, Shape, SpanRef, Value, MAX_NAME_BYTES, MAX_TEXT_BYTES,
};
#[cfg(feature = "otel")]
pub use infrastructure::{OtelSettings, OtelSink, OtelUnavailable};
