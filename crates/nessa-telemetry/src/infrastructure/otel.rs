//! The OpenTelemetry sink: spans and the points inside them, over OTLP/HTTP.
//!
//! A span's trace and span identities are derived from the caller's names
//! ([`SpanRef`]): the first 16 bytes of the SHA-256 of the trace key are the
//! trace id, the first 8 of the trace and span keys together the span id. So
//! a parent or a link is named, never looked up, and a span ended after a
//! restart, or linked from another conversation, joins the trace it names.
//!
//! An open span is held until its end, at most [`OtelSink::MOST_OPEN`] at
//! once; a start past that ends the oldest span of the most crowded name,
//! marked `telemetry.abandoned`. A second start of a span already open is ignored,
//! and so is an end with no start. A point
//! inside an open span is added to it as a span event; any other point, and
//! every measurement, is not wanted yet.
use crate::{
    application::Sink,
    domain::{Event, Shape, SpanRef, Value},
};
use opentelemetry::{
    trace::{
        Link, Span as _, SpanContext, SpanId, SpanKind, TraceContextExt as _, TraceFlags, TraceId,
        TraceState, Tracer as _, TracerProvider as _,
    },
    Context, KeyValue,
};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator, SdkTracerProvider, Span, Tracer};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

thread_local! {
    /// The identities the next span started on this thread is given.
    static NAMED: Cell<Option<(TraceId, SpanId)>> = const { Cell::new(None) };
}

/// Gives a span the identities its [`SpanRef`] names. The SDK asks its id
/// generator for them while it starts a span, on the starting thread, so
/// [`OtelSink::start`] sets them just before and takes them back after; a
/// span started any other way gets random ones.
#[derive(Debug, Default)]
struct NamedIds(RandomIdGenerator);

impl IdGenerator for NamedIds {
    fn new_trace_id(&self) -> TraceId {
        NAMED
            .with(Cell::get)
            .map_or_else(|| self.0.new_trace_id(), |(trace, _)| trace)
    }

    fn new_span_id(&self) -> SpanId {
        NAMED
            .with(Cell::get)
            .map_or_else(|| self.0.new_span_id(), |(_, span)| span)
    }
}

/// Where an [`OtelSink`] sends, and how it names this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtelSettings {
    /// The full traces URL, such as `http://localhost:4318/v1/traces`. None
    /// leaves it to the standard `OTEL_EXPORTER_OTLP_*` variables, which the
    /// exporter reads itself, and their default.
    pub endpoint: Option<String>,
    /// `service.name` on every span.
    pub service: String,
}

/// Why an [`OtelSink`] could not start.
#[derive(Debug)]
pub struct OtelUnavailable(String);

impl std::fmt::Display for OtelUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "the OTLP exporter could not start: {}", self.0)
    }
}
impl std::error::Error for OtelUnavailable {}

/// Spans over OTLP/HTTP, batched off the caller's thread.
pub struct OtelSink {
    provider: SdkTracerProvider,
    tracer: Tracer,
    /// Each open span, with when it was started among the others.
    open: Mutex<HashMap<SpanRef, Held>>,
    started: AtomicU64,
    /// [`Self::MOST_OPEN`], lower in a test.
    most_open: usize,
}

impl OtelSink {
    /// Most spans held open at once.
    pub const MOST_OPEN: usize = 4096;

    /// A sink sending to the collector `settings` names.
    ///
    /// # Errors
    /// [`OtelUnavailable`] when the exporter cannot be built.
    pub fn new(settings: &OtelSettings) -> Result<Self, OtelUnavailable> {
        use opentelemetry_otlp::WithExportConfig;
        // The exporter's HTTP client is reqwest built without a TLS provider
        // of its own: the process's is ring, as for every other client.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let builder = opentelemetry_otlp::SpanExporter::builder().with_http();
        let builder = match &settings.endpoint {
            Some(endpoint) => builder.with_endpoint(endpoint),
            None => builder,
        };
        let exporter = builder
            .build()
            .map_err(|error| OtelUnavailable(error.to_string()))?;
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_id_generator(NamedIds::default())
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name(settings.service.clone())
                    .build(),
            )
            .build();
        let tracer = provider.tracer(settings.service.clone());
        Ok(Self {
            provider,
            tracer,
            open: Mutex::new(HashMap::new()),
            started: AtomicU64::new(0),
            most_open: Self::MOST_OPEN,
        })
    }

    fn start(
        &self,
        event: &Event,
        span: &SpanRef,
        parent: Option<&SpanRef>,
        links: &[SpanRef],
        started: std::time::SystemTime,
    ) -> Span {
        let mut builder = self
            .tracer
            .span_builder(event.name().to_owned())
            .with_kind(SpanKind::Internal)
            .with_start_time(started)
            .with_attributes(attributes(event));
        if !links.is_empty() {
            builder = builder.with_links(
                links
                    .iter()
                    .map(|link| Link::with_context(context(link)))
                    .collect(),
            );
        }
        let parent = match parent {
            Some(parent) => Context::new().with_remote_span_context(context(parent)),
            None => Context::new(),
        };
        // With a parent the SDK takes the parent's trace, which is the one
        // this span names too; without, it asks for the named one.
        NAMED.with(|named| named.set(Some((trace_id(span), span_id(span)))));
        let started = builder.start_with_context(&self.tracer, &parent);
        NAMED.with(|named| named.set(None));
        started
    }
}

impl Sink for OtelSink {
    fn wants(&self, shape: &Shape) -> bool {
        match shape {
            Shape::SpanStart { .. } | Shape::SpanEnd { .. } | Shape::Span { .. } => true,
            Shape::Point { within } => within.is_some(),
            Shape::Measure { .. } => false,
        }
    }

    fn emit(&self, event: &Event) {
        match event.shape() {
            Shape::SpanStart {
                span,
                parent,
                links,
            } => {
                let Ok(mut open) = self.open.lock() else {
                    return;
                };
                // A span already open keeps its first start: a caller that
                // offers the same work again tells the same span.
                if open.contains_key(span) {
                    return;
                }
                if open.len() >= self.most_open {
                    abandon_oldest(&mut open, event.at());
                }
                let started = self.start(event, span, parent.as_ref(), links, event.at());
                let order = self.started.fetch_add(1, Ordering::Relaxed);
                open.insert(
                    span.clone(),
                    Held {
                        order,
                        name: event.name().to_owned(),
                        span: started,
                    },
                );
            }
            Shape::SpanEnd { span } => {
                let held = self.open.lock().ok().and_then(|mut open| open.remove(span));
                if let Some(Held { span: mut held, .. }) = held {
                    held.set_attributes(attributes(event));
                    held.end_with_timestamp(event.at());
                }
            }
            Shape::Span {
                span,
                parent,
                links,
                started,
            } => {
                let mut finished = self.start(event, span, parent.as_ref(), links, *started);
                finished.end_with_timestamp(event.at());
            }
            Shape::Point {
                within: Some(within),
            } => {
                if let Ok(mut open) = self.open.lock() {
                    if let Some(Held { span, .. }) = open.get_mut(within) {
                        span.add_event_with_timestamp(
                            event.name().to_owned(),
                            event.at(),
                            attributes(event),
                        );
                    }
                }
            }
            Shape::Point { within: None } | Shape::Measure { .. } => {}
        }
    }

    fn flush(&self) {
        let _ = self.provider.force_flush();
    }
}

impl Drop for OtelSink {
    fn drop(&mut self) {
        let _ = self.provider.shutdown();
    }
}

/// An open span, with when it started among the others and its name.
struct Held {
    order: u64,
    name: String,
    span: Span,
}

/// Make room by ending, marked `telemetry.abandoned`, the span started
/// longest ago among those of the name holding the most places: its end was
/// lost (a dropped event, work that never finished). Taken from the most
/// crowded name, so a few long-lived spans (a decision waiting days on a
/// person) are not pushed out by a flood of short ones.
fn abandon_oldest(open: &mut HashMap<SpanRef, Held>, at: std::time::SystemTime) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for held in open.values() {
        *counts.entry(held.name.as_str()).or_default() += 1;
    }
    let Some(crowded) = counts
        .into_iter()
        .max_by_key(|(name, count)| (*count, std::cmp::Reverse(*name)))
        .map(|(name, _)| name.to_owned())
    else {
        return;
    };
    let oldest = open
        .iter()
        .filter(|(_, held)| held.name == crowded)
        .min_by_key(|(_, held)| held.order)
        .map(|(span, _)| span.clone());
    if let Some(Held { mut span, .. }) = oldest.and_then(|oldest| open.remove(&oldest)) {
        span.set_attribute(KeyValue::new("telemetry.abandoned", true));
        span.end_with_timestamp(at);
    }
}

fn digest(parts: &[&str]) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    digest.finalize().into()
}

/// Never the all-zero id OTel treats as invalid: a zero digest prefix is
/// astronomically unlikely, and is moved to one if it happens.
fn trace_id(span: &SpanRef) -> TraceId {
    let bytes: [u8; 16] = digest(&[span.trace()])[..16]
        .try_into()
        .expect("sixteen bytes");
    let id = TraceId::from_bytes(bytes);
    if id == TraceId::INVALID {
        TraceId::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
    } else {
        id
    }
}

fn span_id(span: &SpanRef) -> SpanId {
    let bytes: [u8; 8] = digest(&[span.trace(), span.span()])[..8]
        .try_into()
        .expect("eight bytes");
    let id = SpanId::from_bytes(bytes);
    if id == SpanId::INVALID {
        SpanId::from_bytes([0, 0, 0, 0, 0, 0, 0, 1])
    } else {
        id
    }
}

fn context(span: &SpanRef) -> SpanContext {
    SpanContext::new(
        trace_id(span),
        span_id(span),
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    )
}

fn attributes(event: &Event) -> Vec<KeyValue> {
    event
        .attributes()
        .iter()
        .map(|(key, value)| {
            let key = key.to_owned();
            match value {
                Value::Text(text) => KeyValue::new(key, text.to_string()),
                Value::Int(number) => KeyValue::new(key, *number),
                Value::Float(number) => KeyValue::new(key, *number),
                Value::Bool(flag) => KeyValue::new(key, *flag),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        time::{Duration, SystemTime},
    };

    /// One OTLP request, read whole: its request line and body.
    fn collector() -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            let mut path = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if path.is_empty() {
                    path = line.clone();
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
            (path, body)
        });
        (format!("http://{address}/v1/traces"), received)
    }

    #[test]
    fn spans_and_their_points_reach_the_collector_with_their_attributes() {
        let (endpoint, received) = collector();
        let sink = OtelSink::new(&OtelSettings {
            endpoint: Some(endpoint),
            service: "telemetry-test".into(),
        })
        .unwrap();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let turn = SpanRef::new("conversation-a", "turn-1");
        sink.emit(
            &Event::span_start("turn", turn.clone(), start)
                .link(SpanRef::new("conversation-b", "turn-7"))
                .attr("origin.kind", "agent"),
        );
        sink.emit(
            &Event::point("noted", start + Duration::from_millis(5))
                .within(turn.clone())
                .attr("point.attribute", true),
        );
        sink.emit(
            &Event::span(
                "tool",
                SpanRef::new("conversation-a", "tool-1"),
                start,
                start + Duration::from_millis(9),
            )
            .parent(turn.clone()),
        );
        sink.emit(&Event::point("nowhere", start));
        let named = context(&turn);
        sink.emit(
            &Event::span_end("turn", turn, start + Duration::from_millis(10))
                .attr("turn.outcome", "settled"),
        );
        sink.flush();
        let (path, body) = received.join().unwrap();
        assert!(path.starts_with("POST /v1/traces "), "{path}");
        // The turn carries the identities its names give, not random ones.
        let holds = |bytes: &[u8]| body.windows(bytes.len()).any(|window| window == bytes);
        assert!(holds(&named.trace_id().to_bytes()));
        assert!(holds(&named.span_id().to_bytes()));
        let body = String::from_utf8_lossy(&body);
        for expected in [
            "telemetry-test",
            "origin.kind",
            "agent",
            "noted",
            "point.attribute",
            "tool",
            "turn.outcome",
            "settled",
        ] {
            assert!(body.contains(expected), "{expected} in {body}");
        }
        assert!(!body.contains("nowhere"), "{body}");
    }

    /// A span whose end never came is ended to make room, marked so, from
    /// the name holding the most places; a second start of an open span keeps
    /// the first.
    #[test]
    fn a_full_sink_abandons_the_most_crowded_names_oldest_span_and_a_restart_keeps_the_first() {
        let (endpoint, received) = collector();
        let mut sink = OtelSink::new(&OtelSettings {
            endpoint: Some(endpoint),
            service: "telemetry-test".into(),
        })
        .unwrap();
        sink.most_open = 3;
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let span = |name: &'static str| SpanRef::new("trace", name);
        // One long-lived span of another name, oldest of all, stays.
        sink.emit(&Event::span_start("decision", span("waiting"), at));
        sink.emit(&Event::span_start("turn", span("a"), at).attr("first", true));
        sink.emit(&Event::span_start("turn", span("a"), at).attr("second", true));
        sink.emit(&Event::span_start("turn", span("b"), at));
        assert_eq!(sink.open.lock().unwrap().len(), 3);
        // Full: the oldest turn goes, not the older decision.
        sink.emit(&Event::span_start("turn", span("c"), at));
        let open = sink
            .open
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert!(!open.contains(&span("a")), "{open:?}");
        assert!(open.contains(&span("waiting")), "{open:?}");
        assert_eq!(open.len(), 3);
        sink.flush();
        let (_, body) = received.join().unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("telemetry.abandoned"), "{body}");
        assert!(body.contains("first") && !body.contains("second"), "{body}");
    }

    #[test]
    fn the_same_names_give_the_same_identities() {
        let a = SpanRef::new("trace", "span");
        assert_eq!(trace_id(&a), trace_id(&SpanRef::new("trace", "other")));
        assert_eq!(span_id(&a), span_id(&SpanRef::new("trace", "span")));
        assert_ne!(span_id(&a), span_id(&SpanRef::new("trace", "other")));
        assert_ne!(span_id(&a), span_id(&SpanRef::new("trace2", "span")));
    }

    #[test]
    fn only_spans_and_points_inside_them_are_wanted() {
        let sink = OtelSink::new(&OtelSettings {
            endpoint: Some("http://127.0.0.1:9/v1/traces".into()),
            service: "telemetry-test".into(),
        })
        .unwrap();
        assert!(sink.wants(&Shape::Point {
            within: Some(SpanRef::new("t", "s"))
        }));
        assert!(!sink.wants(&Shape::Point { within: None }));
        assert!(!sink.wants(&Shape::Measure {
            value: 1.0,
            kind: crate::domain::MeasureKind::Counter
        }));
    }
}
