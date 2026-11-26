//! The emit point and the sink port.
mod sink;
mod telemetry;

pub use sink::Sink;
pub use telemetry::{Telemetry, DEFAULT_CAPACITY};
