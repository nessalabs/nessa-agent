//! What one telemetry event says: a name, a shape, when, and its attributes.
//!
//! The set of events is open. A caller names an event and chooses its
//! attributes; nothing here lists which events exist. The shape says how a
//! sink should read it — part of a span, a happening at one moment, or a
//! measurement — and a sink keeps the shapes it can carry.
//!
//! Every value is bounded where it is made: a name, a key or a text value
//! past its limit is cut at a character boundary, and attributes past
//! [`Attributes::MAX`] are left off. An event cannot hold an unbounded
//! value, so no sink has to check for one (`limits_are_held_where_values_are_made`).
use std::{borrow::Cow, time::SystemTime};

/// Most UTF-8 bytes of an event name or an attribute key.
pub const MAX_NAME_BYTES: usize = 128;
/// Most UTF-8 bytes of a text attribute value, or of a trace or span key.
pub const MAX_TEXT_BYTES: usize = 1024;

/// `text` cut to at most `max` UTF-8 bytes, at a character boundary.
fn bounded(text: Cow<'static, str>, max: usize) -> Cow<'static, str> {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(text[..end].to_owned())
}

/// One attribute value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Text, at most [`MAX_TEXT_BYTES`].
    Text(Cow<'static, str>),
    /// A whole number.
    Int(i64),
    /// A number with a fraction.
    Float(f64),
    /// Yes or no.
    Bool(bool),
}

impl From<&'static str> for Value {
    fn from(text: &'static str) -> Self {
        Self::Text(bounded(Cow::Borrowed(text), MAX_TEXT_BYTES))
    }
}
impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::Text(bounded(Cow::Owned(text), MAX_TEXT_BYTES))
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}
impl From<u64> for Value {
    /// Past `i64::MAX` it is held as `i64::MAX`.
    fn from(value: u64) -> Self {
        Self::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::from(u64::try_from(value).unwrap_or(u64::MAX))
    }
}
impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Self::Int(i64::from(value))
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

/// An event's attributes, in the order they were added. A key added twice
/// keeps both; a sink reads the last.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attributes(Vec<(Cow<'static, str>, Value)>);

impl Attributes {
    /// Most attributes on one event. Past it, more are left off.
    pub const MAX: usize = 64;

    /// No attributes.
    pub fn new() -> Self {
        Self::default()
    }
    /// Add `key` = `value`, unless [`Self::MAX`] are held already.
    pub fn push(&mut self, key: impl Into<Cow<'static, str>>, value: impl Into<Value>) {
        if self.0.len() < Self::MAX {
            self.0
                .push((bounded(key.into(), MAX_NAME_BYTES), value.into()));
        }
    }
    /// Each key and value, in the order added.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(key, value)| (key.as_ref(), value))
    }
    /// How many are held.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether none are held.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A span, named by the trace it belongs to and its own key in it. Both are
/// the caller's stable names, such as a conversation and a turn: the same
/// names always mean the same span, so a span can be a parent or a link
/// without anyone keeping the identities a sink gave it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpanRef {
    trace: Box<str>,
    span: Box<str>,
}

impl SpanRef {
    /// The span `span` in the trace `trace`, each cut to [`MAX_TEXT_BYTES`].
    pub fn new(trace: impl Into<Cow<'static, str>>, span: impl Into<Cow<'static, str>>) -> Self {
        Self {
            trace: bounded(trace.into(), MAX_TEXT_BYTES).into_owned().into(),
            span: bounded(span.into(), MAX_TEXT_BYTES).into_owned().into(),
        }
    }
    /// The trace's key.
    pub fn trace(&self) -> &str {
        &self.trace
    }
    /// The span's key in it.
    pub fn span(&self) -> &str {
        &self.span
    }
}

/// How a measurement is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureKind {
    /// Added to a running total.
    Counter,
    /// The value now, replacing the last one.
    Gauge,
}

/// How a sink should read an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// A span begins at the event's time.
    SpanStart {
        /// The span.
        span: SpanRef,
        /// The span it starts under, if any.
        parent: Option<SpanRef>,
        /// Spans it follows from.
        links: Vec<SpanRef>,
    },
    /// The span begun by a [`Shape::SpanStart`] ends at the event's time; the
    /// event's attributes are added to it as it ends.
    SpanEnd {
        /// The span.
        span: SpanRef,
    },
    /// A span already over: it began at `started` and ended at the event's
    /// time.
    Span {
        /// The span.
        span: SpanRef,
        /// The span it ran under, if any.
        parent: Option<SpanRef>,
        /// Spans it followed from.
        links: Vec<SpanRef>,
        /// When it began.
        started: SystemTime,
    },
    /// Something that happened at one moment, inside `within` when it is set.
    Point {
        /// The span it happened in, if any.
        within: Option<SpanRef>,
    },
    /// A number measured at the event's time.
    Measure {
        /// The number.
        value: f64,
        /// How it is read.
        kind: MeasureKind,
    },
}

/// One telemetry event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    name: Cow<'static, str>,
    shape: Shape,
    at: SystemTime,
    attributes: Attributes,
}

impl Event {
    /// An event named `name` (cut to [`MAX_NAME_BYTES`]) of `shape`, at `at`.
    /// The caller reads its own clock: this crate reads none.
    pub fn new(name: impl Into<Cow<'static, str>>, shape: Shape, at: SystemTime) -> Self {
        Self {
            name: bounded(name.into(), MAX_NAME_BYTES),
            shape,
            at,
            attributes: Attributes::new(),
        }
    }
    /// A span named `name` begins.
    pub fn span_start(name: impl Into<Cow<'static, str>>, span: SpanRef, at: SystemTime) -> Self {
        Self::new(
            name,
            Shape::SpanStart {
                span,
                parent: None,
                links: Vec::new(),
            },
            at,
        )
    }
    /// The span `span`, named `name`, ends.
    pub fn span_end(name: impl Into<Cow<'static, str>>, span: SpanRef, at: SystemTime) -> Self {
        Self::new(name, Shape::SpanEnd { span }, at)
    }
    /// A span named `name` that ran from `started` to `ended`.
    pub fn span(
        name: impl Into<Cow<'static, str>>,
        span: SpanRef,
        started: SystemTime,
        ended: SystemTime,
    ) -> Self {
        Self::new(
            name,
            Shape::Span {
                span,
                parent: None,
                links: Vec::new(),
                started,
            },
            ended,
        )
    }
    /// Something named `name` happened at `at`.
    pub fn point(name: impl Into<Cow<'static, str>>, at: SystemTime) -> Self {
        Self::new(name, Shape::Point { within: None }, at)
    }
    /// `value`, named `name`, measured at `at`.
    pub fn measure(
        name: impl Into<Cow<'static, str>>,
        value: f64,
        kind: MeasureKind,
        at: SystemTime,
    ) -> Self {
        Self::new(name, Shape::Measure { value, kind }, at)
    }

    /// The span this one starts under. Only a span start or a finished span
    /// has one; on any other shape it is ignored.
    pub fn parent(mut self, parent: SpanRef) -> Self {
        if let Shape::SpanStart { parent: held, .. } | Shape::Span { parent: held, .. } =
            &mut self.shape
        {
            *held = Some(parent);
        }
        self
    }
    /// A span this one follows from, such as the turn that sent its input.
    /// Only a span start or a finished span has links.
    pub fn link(mut self, link: SpanRef) -> Self {
        if let Shape::SpanStart { links, .. } | Shape::Span { links, .. } = &mut self.shape {
            links.push(link);
        }
        self
    }
    /// The span a point happened inside. Only a point has one.
    pub fn within(mut self, span: SpanRef) -> Self {
        if let Shape::Point { within } = &mut self.shape {
            *within = Some(span);
        }
        self
    }
    /// Add `key` = `value` (see [`Attributes::push`]).
    pub fn attr(mut self, key: impl Into<Cow<'static, str>>, value: impl Into<Value>) -> Self {
        self.attributes.push(key, value);
        self
    }
    /// Add `key` = `value` when there is one.
    pub fn attr_some(
        self,
        key: impl Into<Cow<'static, str>>,
        value: Option<impl Into<Value>>,
    ) -> Self {
        match value {
            Some(value) => self.attr(key, value),
            None => self,
        }
    }

    /// The event's name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// How it should be read.
    pub fn shape(&self) -> &Shape {
        &self.shape
    }
    /// When it happened, or when a span began or ended.
    pub fn at(&self) -> SystemTime {
        self.at
    }
    /// Its attributes.
    pub fn attributes(&self) -> &Attributes {
        &self.attributes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_held_where_values_are_made() {
        let long = "é".repeat(MAX_TEXT_BYTES);
        let event = Event::point("n".repeat(MAX_NAME_BYTES + 5), SystemTime::UNIX_EPOCH)
            .attr("k".repeat(MAX_NAME_BYTES + 1), long.clone());
        assert_eq!(event.name().len(), MAX_NAME_BYTES);
        let (key, value) = event.attributes().iter().next().unwrap();
        assert_eq!(key.len(), MAX_NAME_BYTES);
        let Value::Text(text) = value else {
            panic!("text stays text")
        };
        assert!(text.len() <= MAX_TEXT_BYTES);
        assert!(
            long.starts_with(text.as_ref()),
            "cut at a character boundary"
        );

        let mut many = Event::point("many", SystemTime::UNIX_EPOCH);
        for index in 0..Attributes::MAX + 3 {
            many = many.attr("index", index);
        }
        assert_eq!(many.attributes().len(), Attributes::MAX);

        let span = SpanRef::new("t".repeat(MAX_TEXT_BYTES + 1), "s");
        assert_eq!(span.trace().len(), MAX_TEXT_BYTES);
    }

    #[test]
    fn parent_links_and_within_apply_only_to_shapes_that_have_them() {
        let span = SpanRef::new("trace", "a");
        let point = Event::point("p", SystemTime::UNIX_EPOCH)
            .parent(span.clone())
            .link(span.clone());
        assert_eq!(point.shape(), &Shape::Point { within: None });
        let start = Event::span_start("s", SpanRef::new("trace", "b"), SystemTime::UNIX_EPOCH)
            .parent(span.clone())
            .link(span.clone())
            .within(span.clone());
        assert_eq!(
            start.shape(),
            &Shape::SpanStart {
                span: SpanRef::new("trace", "b"),
                parent: Some(span.clone()),
                links: vec![span],
            }
        );
    }

    #[test]
    fn a_large_unsigned_number_is_held_at_the_largest_whole_number() {
        assert_eq!(Value::from(u64::MAX), Value::Int(i64::MAX));
    }
}
