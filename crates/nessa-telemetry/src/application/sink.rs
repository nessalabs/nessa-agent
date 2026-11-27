//! Where events go: one sink per backend.
use crate::domain::{Event, Shape};

/// One backend events are sent to, such as an OpenTelemetry collector.
///
/// The dispatcher calls a sink from its own thread, one event at a time and
/// in the order they were emitted, and never from the thread that emitted
/// them. A sink may take its time, but a slow one holds back the others: send
/// in batches rather than one request per event. A sink that panics loses
/// that event and keeps the rest (`a_panicking_sink_loses_one_event_and_keeps_the_rest`).
pub trait Sink: Send + Sync {
    /// Whether this sink keeps events of `shape`. One that does not is never
    /// handed them.
    fn wants(&self, shape: &Shape) -> bool {
        let _ = shape;
        true
    }
    /// Take one event. Losing it is acceptable; blocking the caller is not
    /// possible here, because the caller is the dispatcher's thread.
    fn emit(&self, event: &Event);
    /// Send what is held, before the process stops. Bounded by the sink's own
    /// timeout.
    fn flush(&self) {}
}
