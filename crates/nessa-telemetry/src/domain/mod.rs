//! What an event says.
mod event;

pub use event::{
    Attributes, Event, MeasureKind, Shape, SpanRef, Value, MAX_NAME_BYTES, MAX_TEXT_BYTES,
};
