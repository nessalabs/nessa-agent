//! Server-sent events, bounded before they are buffered. One event is
//! `field: value` lines ended by a blank line. Comment and empty events are
//! skipped by the caller; they are not JSON-RPC.
//!
//! Bytes are counted as they arrive. An event that would pass
//! [`MAX_FRAME_BYTES`](super::framing::MAX_FRAME_BYTES) is refused without
//! keeping the excess (`a_split_sse_event_past_the_frame_bound_is_refused`).
use super::framing::MAX_FRAME_BYTES;

/// Why the event stream cannot continue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SseError {
    /// An event grew past the frame bound.
    TooLarge,
    /// A completed event's data is not UTF-8.
    Utf8,
}

/// One SSE event. `event` is empty when the peer omitted the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SseEvent {
    /// The `event:` field, or empty.
    pub(crate) event: String,
    /// The `data:` lines joined with newlines, without the trailing break.
    pub(crate) data: String,
}

/// A parser that holds only a partial event.
pub(crate) struct SseParser {
    pending: Vec<u8>,
    limit: usize,
}

impl SseParser {
    /// A parser that refuses an event past `limit` bytes.
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            pending: Vec::new(),
            limit,
        }
    }

    /// The frame bound used for MCP event bodies.
    pub(crate) fn bounded() -> Self {
        Self::new(MAX_FRAME_BYTES)
    }

    /// Take complete events out of `chunk`. A partial event stays here.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let mut events = Vec::new();
        for byte in chunk {
            if self.pending.len() >= self.limit {
                self.pending.clear();
                return Err(SseError::TooLarge);
            }
            self.pending.push(*byte);
            if self.pending.ends_with(b"\n\n") || self.pending.ends_with(b"\r\n\r\n") {
                let raw = std::mem::take(&mut self.pending);
                if let Some(event) = parse_event(&raw)? {
                    events.push(event);
                }
            }
        }
        Ok(events)
    }
}

fn parse_event(raw: &[u8]) -> Result<Option<SseEvent>, SseError> {
    let text = std::str::from_utf8(raw).map_err(|_| SseError::Utf8)?;
    let mut event = String::new();
    let mut data: Vec<&str> = Vec::new();
    for line in text.split(['\n', '\r']).filter(|line| !line.is_empty()) {
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = value.to_owned(),
            "data" => data.push(value),
            _ => {}
        }
    }
    if event.is_empty() && data.is_empty() {
        return Ok(None);
    }
    Ok(Some(SseEvent {
        event,
        data: data.join("\n"),
    }))
}

/// Whether `content_type` is an SSE body.
pub(crate) fn is_event_stream(content_type: Option<&str>) -> bool {
    content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/event-stream")
}

/// Whether `content_type` is a JSON body.
pub(crate) fn is_json(content_type: Option<&str>) -> bool {
    let media = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    media.eq_ignore_ascii_case("application/json")
        || media.eq_ignore_ascii_case("application/json-rpc")
}
