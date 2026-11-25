//! Experimental direct client for one selected MCP Events Beta poll stream.
//! No session, agent event, callback, runtime poll loop, or durable cursor.
#![deny(missing_docs)]
use super::authorization::{Bearer, RemoteAuthorization};
use super::http_exchange::{BodyRead, HttpBody, HttpMethod, HttpRequest, HttpResponse};
use super::remote::RemoteMcpServer;
use super::servers::{EventsAdmission, McpOwner, McpServers};
use super::sse::{is_event_stream, is_json, SseParser};
use super::McpError;
use crate::infrastructure::clock::within;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::io::{self, Write};
use std::time::Duration;

const VERSION: &str = "2026-07-28";
const EXTENSION: &str = "io.modelcontextprotocol/events";
const BODY_LIMIT: usize = 256 * 1024;
const MAX_PAGES: usize = 8;
const MAX_EVENTS: usize = 32;
const MAX_NAME: usize = 256;
const MAX_CURSOR: usize = 4096;
const BUDGET: Duration = Duration::from_secs(15);

/// Refusal from the opt-in MCP Events Beta poll subset. No remote text is returned.
#[derive(Debug, PartialEq, Eq)]
pub enum ExperimentalEventsError {
    /// The host boundary refused or the transport did not reach an answer.
    Mcp(McpError),
    /// The server does not advertise the pinned Events revision and extension.
    Unsupported,
    /// The selected event does not advertise Poll or was not found.
    PollUnavailable,
    /// This subset does not forward schema-declared routing arguments as HTTP headers.
    RoutingArgumentsUnsupported,
    /// The request or response exceeds a local operation bound.
    Limit,
    /// The upstream rejected this request by JSON-RPC status or HTTP status.
    Rejected(i64),
    /// The peer failed to return one complete result matching the request.
    InvalidReply,
    /// Replay capability changed on this handle. Reopen explicitly to change mode.
    ReplayModeChanged,
}

impl From<McpError> for ExperimentalEventsError {
    fn from(error: McpError) -> Self {
        Self::Mcp(error)
    }
}

/// One untrusted event occurrence returned by the selected upstream event type.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentalEvent {
    /// Upstream identifier (deduplication is owned by the host per subscription).
    pub event_id: String,
    /// Upstream timestamp, without a local ordering or clock guarantee.
    pub timestamp: String,
    /// Untrusted data, not instructions. The host owns any schema or use decision.
    pub data: Map<String, Value>,
    name: String,
}

/// Result of one explicitly caller-driven poll.
pub struct ExperimentalEventsBatch {
    /// Occurrences from this completed response.
    pub events: Vec<ExperimentalEvent>,
    /// The upstream advanced past the supplied checkpoint.
    pub gap: bool,
    /// The upstream says this batch does not exhaust the available occurrences.
    pub has_more: bool,
    /// Advisory minimum interval in milliseconds, with a local floor.
    pub next_poll_ms: u64,
    /// Whether the next poll has an opaque checkpoint. No persistence is provided.
    pub replay_available: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollReply {
    events: Vec<ExperimentalEvent>,
    cursor: Option<String>,
    truncated: bool,
    has_more: bool,
    next_poll_ms: u64,
}

/// The handle owns the established replay mode and its last accepted position.
#[derive(Debug, PartialEq, Eq)]
enum PollPosition {
    Unclassified,
    NonReplay,
    Replay(String),
}

impl PollPosition {
    fn cursor(&self) -> Option<&str> {
        match self {
            Self::Unclassified | Self::NonReplay => None,
            Self::Replay(cursor) => Some(cursor),
        }
    }

    fn accept(
        &self,
        name: &str,
        reply: PollReply,
    ) -> Result<(Self, ExperimentalEventsBatch), ExperimentalEventsError> {
        if reply.events.len() > MAX_EVENTS
            || reply.cursor.as_ref().is_some_and(|c| c.len() > MAX_CURSOR)
        {
            return Err(ExperimentalEventsError::Limit);
        }
        if reply
            .events
            .iter()
            .any(|event| event.name != name || event.event_id.is_empty())
        {
            return Err(ExperimentalEventsError::InvalidReply);
        }
        let (position, gap) = match (self, reply.cursor) {
            (Self::Unclassified | Self::NonReplay, None) => (Self::NonReplay, false),
            (Self::Unclassified, Some(cursor)) if reply.events.is_empty() => {
                (Self::Replay(cursor), false)
            }
            (Self::Unclassified, Some(_)) => return Err(ExperimentalEventsError::InvalidReply),
            (Self::Replay(_), Some(cursor)) => (Self::Replay(cursor), reply.truncated),
            (Self::NonReplay, Some(_)) | (Self::Replay(_), None) => {
                return Err(ExperimentalEventsError::ReplayModeChanged);
            }
        };
        let batch = ExperimentalEventsBatch {
            events: reply.events,
            gap,
            has_more: reply.has_more,
            next_poll_ms: reply.next_poll_ms.max(1000),
            replay_available: matches!(position, Self::Replay(_)),
        };
        Ok((position, batch))
    }
}

enum Call<'a> {
    Discover,
    List(Option<&'a str>),
    Poll,
}

#[derive(Serialize)]
struct Request<T> {
    jsonrpc: &'static str,
    id: u64,
    method: &'static str,
    params: RequestParams<T>,
}

#[derive(Serialize)]
struct RequestParams<T> {
    #[serde(rename = "_meta")]
    metadata: Value,
    #[serde(flatten)]
    fields: T,
}

#[derive(Serialize)]
struct ListParams<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PollParams<'a> {
    name: &'a str,
    arguments: &'a Map<String, Value>,
    cursor: Option<&'a str>,
    max_events: usize,
}

struct BoundedBody(Vec<u8>);

impl Write for BoundedBody {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > BODY_LIMIT - self.0.len() {
            return Err(io::Error::other("MCP Events body limit"));
        }
        let needed = self.0.len() + data.len();
        if self.0.capacity() < needed {
            let capacity = self
                .0
                .capacity()
                .saturating_mul(2)
                .max(needed)
                .min(BODY_LIMIT);
            self.0.reserve_exact(capacity - self.0.len());
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode<T: Serialize>(
    id: u64,
    method: &'static str,
    fields: T,
) -> Result<Vec<u8>, ExperimentalEventsError> {
    let metadata = json!({
        "io.modelcontextprotocol/protocolVersion":VERSION,
        "io.modelcontextprotocol/clientInfo":{"name":"Nessa","version":env!("CARGO_PKG_VERSION")},
        "io.modelcontextprotocol/clientCapabilities":{"extensions":{(EXTENSION):{}}},
    });
    let mut bytes = BoundedBody(Vec::new());
    serde_json::to_writer(
        &mut bytes,
        &Request {
            jsonrpc: "2.0",
            id,
            method,
            params: RequestParams { metadata, fields },
        },
    )
    .map_err(|error| {
        if error.is_io() {
            ExperimentalEventsError::Limit
        } else {
            ExperimentalEventsError::InvalidReply
        }
    })?;
    Ok(bytes.0)
}

fn poll_params<'a>(
    name: &'a str,
    arguments: &'a Map<String, Value>,
    cursor: Option<&'a str>,
) -> PollParams<'a> {
    PollParams {
        name,
        arguments,
        cursor,
        max_events: MAX_EVENTS,
    }
}

/// Caller-owned, opt-in handle for one event, with one request at a time.
/// Dropping it drops the checkpoint; it has no tasks to stop.
pub struct ExperimentalEventsPoll {
    servers: McpServers,
    admission: EventsAdmission,
    owner: McpOwner,
    next_id: u64,
    name: String,
    arguments: Map<String, Value>,
    position: PollPosition,
    credential: Option<Option<u64>>,
}

impl ExperimentalEventsPoll {
    pub(super) async fn open(
        servers: McpServers,
        admitted: &RemoteMcpServer,
        owner: McpOwner,
        name: String,
        arguments: Map<String, Value>,
    ) -> Result<Self, ExperimentalEventsError> {
        if name.is_empty() || name.len() > MAX_NAME {
            return Err(ExperimentalEventsError::Limit);
        }
        let admission = servers.events_admit(admitted, &owner)?;
        // Validate before discovery. A full-length control-character cursor
        // expands to six JSON bytes per input byte; this covers every later
        // admitted cursor as well as the widest possible request id.
        let largest_cursor = "\0".repeat(MAX_CURSOR);
        encode(
            u64::MAX,
            "events/poll",
            poll_params(&name, &arguments, Some(&largest_cursor)),
        )?;
        let mut handle = Self {
            servers,
            admission,
            owner,
            next_id: 0,
            name,
            arguments,
            position: PollPosition::Unclassified,
            credential: None,
        };
        let clock = handle.admission.clock.clone();
        let deadline = clock.now() + BUDGET;
        within(clock.as_ref(), deadline, handle.discover_selected())
            .await
            .ok_or(McpError::Timeout)??;
        Ok(handle)
    }

    async fn discover_selected(&mut self) -> Result<(), ExperimentalEventsError> {
        let reply = self.post(Call::Discover).await?;
        if !reply["supportedVersions"]
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s == VERSION))
            || !reply["capabilities"]["extensions"][EXTENSION].is_object()
        {
            return Err(ExperimentalEventsError::Unsupported);
        }

        let mut cursor = None::<String>;
        let mut selected = None;
        for _ in 0..MAX_PAGES {
            let reply = self.post(Call::List(cursor.as_deref())).await?;
            let items = reply["events"]
                .as_array()
                .ok_or(ExperimentalEventsError::InvalidReply)?;
            for item in items {
                if item["name"] == self.name && selected.replace(item.clone()).is_some() {
                    return Err(ExperimentalEventsError::InvalidReply);
                }
            }
            cursor = match reply.get("nextCursor") {
                None => return Self::finish_selection(selected),
                Some(Value::String(value))
                    if value.len() <= MAX_CURSOR && Some(value) != cursor.as_ref() =>
                {
                    Some(value.clone())
                }
                _ => return Err(ExperimentalEventsError::InvalidReply),
            };
        }
        Err(ExperimentalEventsError::Limit)
    }

    fn finish_selection(item: Option<Value>) -> Result<(), ExperimentalEventsError> {
        let item = item.ok_or(ExperimentalEventsError::PollUnavailable)?;
        if !item["delivery"]
            .as_array()
            .is_some_and(|delivery| delivery.iter().any(|mode| mode == "poll"))
        {
            return Err(ExperimentalEventsError::PollUnavailable);
        }
        let schema = &item["inputSchema"];
        if !schema.is_object() || !item["payloadSchema"].is_object() {
            return Err(ExperimentalEventsError::InvalidReply);
        }
        // Nested properties can carry routing rules, too. Exclude even
        // annotations in unsupported schema positions rather than risk an
        // unmirrored request. The upstream schema is already bounded JSON.
        if has_routing_annotation(schema) {
            return Err(ExperimentalEventsError::RoutingArgumentsUnsupported);
        }
        Ok(())
    }

    /// Send one poll using the checkpoint owned by this handle. The first poll
    /// starts at the current upstream position. The caller controls any follow-up.
    ///
    /// # Errors
    ///
    /// Returns a typed refusal for failed admission, transport, or invalid result.
    /// Revocation fences acceptance; a request already sent may still reach the peer.
    /// HTTP 401 is not automatically retried. Scope refusals are reported to the
    /// existing authorization owner without replaying the request. A reply that
    /// changes an established replay mode is refused without advancing the
    /// position; open a fresh handle explicitly to accept a new mode.
    pub async fn poll(&mut self) -> Result<ExperimentalEventsBatch, ExperimentalEventsError> {
        let clock = self.admission.clock.clone();
        let deadline = clock.now() + BUDGET;
        let result = within(clock.as_ref(), deadline, self.post(Call::Poll))
            .await
            .ok_or(McpError::Timeout)??;
        let reply: PollReply =
            serde_json::from_value(result).map_err(|_| ExperimentalEventsError::InvalidReply)?;
        let (position, batch) = self.position.accept(&self.name, reply)?;
        self.servers.events_check(&self.admission, &self.owner)?;
        self.position = position;
        Ok(batch)
    }

    async fn current_bearer(
        &self,
        authorization: &dyn RemoteAuthorization,
    ) -> Result<Option<Bearer>, ExperimentalEventsError> {
        self.servers.events_check(&self.admission, &self.owner)?;
        let bearer = authorization.bearer(self.admission.remote.id()).await?;
        self.servers.events_check(&self.admission, &self.owner)?;
        if self
            .credential
            .is_some_and(|saved| saved != bearer.as_ref().map(Bearer::generation))
        {
            return Err(McpError::Closed.into());
        }
        Ok(bearer)
    }

    async fn post(&mut self, call: Call<'_>) -> Result<Value, ExperimentalEventsError> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ExperimentalEventsError::Limit)?;
        let id = self.next_id;
        let (method, body) = match call {
            Call::Discover => (
                "server/discover",
                encode(id, "server/discover", ListParams { cursor: None })?,
            ),
            Call::List(cursor) => (
                "events/list",
                encode(id, "events/list", ListParams { cursor })?,
            ),
            Call::Poll => (
                "events/poll",
                encode(
                    id,
                    "events/poll",
                    poll_params(&self.name, &self.arguments, self.position.cursor()),
                )?,
            ),
        };
        let bearer = self
            .current_bearer(self.admission.authorization.as_ref())
            .await?;
        self.credential = Some(bearer.as_ref().map(Bearer::generation));
        let mut headers = vec![
            ("Content-Type".into(), "application/json".into()),
            (
                "Accept".into(),
                "application/json, text/event-stream".into(),
            ),
            ("MCP-Protocol-Version".into(), VERSION.into()),
            ("Mcp-Method".into(), method.to_owned()),
        ];
        if matches!(call, Call::Poll) {
            headers.push(("Mcp-Name".into(), header_value(&self.name)));
        }
        if let Some(bearer) = bearer {
            headers.push(("Authorization".into(), format!("Bearer {}", bearer.token())));
        }
        let response = self
            .admission
            .exchange
            .exchange(HttpRequest {
                method: HttpMethod::Post,
                url: self.admission.remote.url().as_str().to_owned(),
                headers,
                body,
            })
            .await;
        self.current_bearer(self.admission.authorization.as_ref())
            .await?;
        let response = response.map_err(|_| McpError::Unconfirmed)?;
        // Match the existing RemoteAuthorization port contract on HTTP 403.
        // Record the challenge once for this admitted response, never retry.
        if response.status == 403 {
            let challenge = response
                .header("www-authenticate")
                .unwrap_or("Bearer")
                .to_owned();
            drop(response);
            self.admission
                .authorization
                .insufficient_scope(self.admission.remote.id(), &challenge)
                .await;
            self.servers.events_check(&self.admission, &self.owner)?;
            return Err(McpError::InsufficientScope.into());
        }
        let result = reply(response, id).await;
        self.current_bearer(self.admission.authorization.as_ref())
            .await?;
        result
    }
}

fn has_routing_annotation(schema: &Value) -> bool {
    match schema {
        Value::Object(fields) => {
            fields.contains_key("x-mcp-header") || fields.values().any(has_routing_annotation)
        }
        Value::Array(elements) => elements.iter().any(has_routing_annotation),
        _ => false,
    }
}

fn header_value(name: &str) -> String {
    if name.trim() == name
        && name.bytes().all(|byte| (b' '..=b'~').contains(&byte))
        && !(name.starts_with("=?base64?") && name.ends_with("?="))
    {
        name.to_owned()
    } else {
        format!("=?base64?{}?=", STANDARD.encode(name))
    }
}

async fn reply(response: HttpResponse, id: u64) -> Result<Value, ExperimentalEventsError> {
    let status = response.status;
    if status == 401 {
        return Err(McpError::Unauthorized.into());
    }
    let content_type = response.header("Content-Type");
    if is_json(content_type) {
        let body = response
            .bytes(BODY_LIMIT)
            .await
            .map_err(|error| match error {
                BodyRead::TooLarge => ExperimentalEventsError::Limit,
                BodyRead::Closed => McpError::Unconfirmed.into(),
            })?;
        let result = decode(&body, id, false)?.ok_or(ExperimentalEventsError::InvalidReply)?;
        if status == 200 {
            return Ok(result);
        }
    } else if status == 200 && is_event_stream(content_type) {
        let mut parser = SseParser::new(BODY_LIMIT);
        let mut remaining = BODY_LIMIT;
        let mut first = match response.body {
            HttpBody::Buffered(body) => {
                return sse_chunk(&mut parser, &mut remaining, &body, id)?
                    .ok_or(ExperimentalEventsError::InvalidReply);
            }
            HttpBody::Stream(stream) => stream,
        };
        while let Some(chunk) = first.next().await.map_err(|_| McpError::Unconfirmed)? {
            if let Some(result) = sse_chunk(&mut parser, &mut remaining, &chunk, id)? {
                return Ok(result);
            }
        }
        return Err(McpError::Unconfirmed.into());
    } else if status == 200 {
        return Err(ExperimentalEventsError::InvalidReply);
    }
    Err(ExperimentalEventsError::Rejected(i64::from(status)))
}

fn sse_chunk(
    parser: &mut SseParser,
    remaining: &mut usize,
    chunk: &[u8],
    id: u64,
) -> Result<Option<Value>, ExperimentalEventsError> {
    *remaining = remaining
        .checked_sub(chunk.len())
        .ok_or(ExperimentalEventsError::Limit)?;
    let mut input = chunk;
    while let Some(event) = parser
        .next_event(&mut input)
        .map_err(|_| ExperimentalEventsError::InvalidReply)?
    {
        if let Some(result) = decode(event.data.as_bytes(), id, true)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

fn decode(raw: &[u8], id: u64, sse: bool) -> Result<Option<Value>, ExperimentalEventsError> {
    let body: Value =
        serde_json::from_slice(raw).map_err(|_| ExperimentalEventsError::InvalidReply)?;
    if body["jsonrpc"] != "2.0" {
        return Err(ExperimentalEventsError::InvalidReply);
    }
    if sse
        && body.get("id").is_none()
        && body.get("result").is_none()
        && body.get("error").is_none()
        && matches!(
            body["method"].as_str(),
            Some("notifications/progress" | "notifications/message")
        )
    {
        return Ok(None);
    }
    if body["id"].as_u64() != Some(id) || body.get("method").is_some() {
        return Err(ExperimentalEventsError::InvalidReply);
    }
    match (body.get("result"), body.get("error")) {
        (Some(result), None) if result["resultType"] == "complete" => Ok(Some(result.clone())),
        (None, Some(error)) => Err(ExperimentalEventsError::Rejected(
            error["code"]
                .as_i64()
                .ok_or(ExperimentalEventsError::InvalidReply)?,
        )),
        _ => Err(ExperimentalEventsError::InvalidReply),
    }
}

#[cfg(test)]
mod position_tests {
    use super::{ExperimentalEventsError, PollPosition, PollReply};
    use serde_json::{json, Value};

    const NAME: &str = "ci.build_finished";

    fn reply(cursor: Option<Value>, event: bool, truncated: bool) -> PollReply {
        let events = if event {
            json!([{"name":NAME,"eventId":"build-1","timestamp":"2026-11-24T00:00:00Z","data":{}}])
        } else {
            json!([])
        };
        let mut body =
            json!({"events":events,"truncated":truncated,"hasMore":false,"nextPollMs":1000});
        if let Some(cursor) = cursor {
            body["cursor"] = cursor;
        }
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn wire_cursor_classifies_once_then_advances_or_refuses() {
        use PollPosition::{NonReplay, Replay, Unclassified};
        for absent in [None, Some(Value::Null)] {
            let (nonreplay, first) = Unclassified
                .accept(NAME, reply(absent.clone(), true, false))
                .unwrap();
            assert_eq!(nonreplay, NonReplay);
            assert!(!first.replay_available && !first.gap && first.events.len() == 1);
            let (still_nonreplay, ignored_gap) = nonreplay
                .accept(NAME, reply(absent.clone(), false, true))
                .unwrap();
            assert_eq!(still_nonreplay, NonReplay);
            assert!(!ignored_gap.gap);
            assert!(matches!(
                nonreplay.accept(NAME, reply(Some(json!("new-mode")), false, false)),
                Err(ExperimentalEventsError::ReplayModeChanged)
            ));

            let (replay, bootstrap) = Unclassified
                .accept(NAME, reply(Some(json!("start")), false, false))
                .unwrap();
            assert_eq!(replay.cursor(), Some("start"));
            assert!(bootstrap.replay_available && bootstrap.events.is_empty() && !bootstrap.gap);
            assert!(matches!(
                replay.accept(NAME, reply(absent, false, false)),
                Err(ExperimentalEventsError::ReplayModeChanged)
            ));
            assert_eq!(replay.cursor(), Some("start"));
            let (replayed, regular) = replay
                .accept(NAME, reply(Some(json!("after-build")), true, false))
                .unwrap();
            assert_eq!(replayed, Replay("after-build".into()));
            assert!(!regular.gap && regular.events.len() == 1);
            let (resumed, gap) = replayed
                .accept(NAME, reply(Some(json!("retained-floor")), false, true))
                .unwrap();
            assert_eq!(resumed, Replay("retained-floor".into()));
            assert!(gap.gap && gap.replay_available && gap.events.is_empty());
        }
        assert!(matches!(
            Unclassified.accept(NAME, reply(Some(json!("unexpected")), true, false)),
            Err(ExperimentalEventsError::InvalidReply)
        ));
    }
}
