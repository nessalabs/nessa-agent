//! One protected native connection and one attempt outcome. Facades never own
//! their own connection.
//!
//! The device dials the gateway's native address, proves its key over TLS with
//! the gateway key strictly pinned, selects the product session with a first
//! `openProduct` envelope, and authenticates with its issued credential id.
//! Product messages then travel one per length-prefixed frame (design
//! "Protected reads over the native channel", rows PR1, PR3, PR9).
//!
//! A session may hold one record watch. Its hints are kept in the session's
//! `WatchInbox` whenever they arrive, so an operation waiting for its own
//! response never counts them as unexpected events (committed change watches,
//! rows W6, W10, W15).
use super::deadline_stream::{io_cause, DeadlineStream};
use crate::app::ports::Clock;
use crate::device_pairing::infrastructure::{
    encode_frame,
    wire::{
        decode_reply, encode_request as encode_envelope, NativePairingReply, NativePairingRequest,
    },
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use crate::read_only_sync::application::{
    watch::Wait, Cancellation, GatewayConnector, GatewayError, GatewayOutcome, GatewayPolicy,
    GatewayStream,
};
use nessa_auth::adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport};
use nessa_protocol::product::generated::{
    product_event, product_method, wire_shape_product_session_ready,
    wire_shape_session_authenticate_params, wire_shape_session_challenge, ChangeWatchId,
    ConversationChanged, ConversationWatchEnded, ConversationWatchRecordsParams,
    ConversationWatchResult, ProductClientMetadata, ProductSessionReady, SessionAuthenticateParams,
    SessionChallenge, CHANGE_WATCH_ID_PATTERN, MAX_AUTH_CREDENTIAL_CHARACTERS,
    MAX_CHANGE_WATCH_ID_BYTES, MAX_PRODUCT_CLIENT_ID_CHARACTERS, PRODUCT_HANDSHAKE_METHOD,
    PRODUCT_VERSION,
};
use nessa_protocol::product::handshake::{authentication_close_reason, supports_product_version};
use nessa_protocol::product::passive_read::{encode_request, ReadEncodeError};
use nessa_protocol::product_contract::generated::{
    CatalogueReadErrorCode, ChangeWatchEndReason, ChangeWatchErrorCode, RecordReadErrorCode,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, ResponseFrame};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::io::{ErrorKind, Read, Result as IoResult, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct LocalConnector;
impl GatewayConnector for LocalConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        TcpStream::connect_timeout(&address, timeout)
            .map(|socket| Box::new(Socket(socket)) as Box<dyn GatewayStream>)
    }
}
struct Socket(TcpStream);
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.0.read(bytes)
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.0.flush()
    }
}
impl GatewayStream for Socket {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_read_timeout(Some(timeout))
    }
    fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
        self.0.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) -> IoResult<()> {
        self.0.shutdown(Shutdown::Both)
    }
}

/// What a paired device presents on one protected connection: its key, the
/// gateway key it pinned at enrollment, and its issued credential id. The id
/// is evidence only together with the key; it is not a bearer secret.
pub(crate) struct DeviceEvidence<'a> {
    pub(crate) identity: &'a NativeIdentity,
    pub(crate) pin: [u8; 44],
    pub(crate) credential: &'a str,
}

/// Product frames over the pinned TLS connection.
struct Channel {
    transport: NativeTransport<DeadlineStream>,
    frames: FrameReader,
}
impl Channel {
    fn send(&mut self, text: &str) -> Result<(), GatewayError> {
        let frame = encode_frame(MAX_PROTECTED_REQUEST_BYTES, text.as_bytes())
            .map_err(|_| GatewayError::RequestTooLarge)?;
        let result = self
            .transport
            .write_all(&frame)
            .and_then(|()| self.transport.flush());
        result.map_err(|error| self.io_failure(&error))
    }
    fn receive(&mut self) -> Result<String, GatewayError> {
        loop {
            let buffer = self
                .frames
                .unfilled()
                .map_err(|_| GatewayError::ResponseTooLarge)?;
            let count = match self.transport.read(buffer) {
                Ok(0) => return Err(GatewayError::Closed(None)),
                Ok(count) => count,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(self.io_failure(&error)),
            };
            if let Some(body) = self
                .frames
                .filled(count)
                .map_err(|_| GatewayError::ResponseTooLarge)?
            {
                return String::from_utf8(body).map_err(|_| GatewayError::Protocol);
            }
        }
    }
    /// The cause the stream recorded comes first (a deadline, cancellation,
    /// or physical error it saw); otherwise the error TLS returned, such as
    /// an end without TLS close, by the same classification.
    fn io_failure(&mut self, error: &std::io::Error) -> GatewayError {
        self.transport
            .stream_mut()
            .take_failure()
            .unwrap_or_else(|| io_cause(error))
    }
}

#[derive(Clone, Copy)]
pub(super) enum RpcKind {
    Authentication,
    Record,
    Catalogue,
    Watch,
}

/// What the session holds for its one registered watch: whether any hint
/// arrived since the last wait took it (many hints are one bit), and the
/// watch's end, once the gateway sends it.
struct WatchInbox {
    watch: ChangeWatchId,
    dirty: bool,
    ended: Option<ChangeWatchEndReason>,
}

/// How events are read for one session: the watch's own events go to its
/// inbox; every other event counts toward the operation's capacity.
struct Events {
    policy: GatewayPolicy,
    unexpected: usize,
    inbox: Option<WatchInbox>,
}
impl Events {
    fn new(policy: GatewayPolicy) -> Self {
        Self {
            policy,
            unexpected: 0,
            inbox: None,
        }
    }
    /// A watch event names a watch this connection minted, or the connection
    /// is broken: identities are per connection, so none can be foreign
    /// (rows U2, W15).
    fn observe(&mut self, event: EventFrame) -> Result<(), GatewayError> {
        let watch = match event.event.as_str() {
            product_event::CONVERSATION_CHANGED => {
                strict_decode::<ConversationChanged>(event.payload)?.watch_id
            }
            product_event::CONVERSATION_WATCH_ENDED => {
                let ended = strict_decode::<ConversationWatchEnded>(event.payload)?;
                let inbox = self.own_inbox(&ended.watch_id)?;
                inbox.ended.get_or_insert(ended.reason);
                return Ok(());
            }
            _ => {
                if self.unexpected >= self.policy.unexpected_events() {
                    return Err(GatewayError::EventCapacity);
                }
                self.unexpected += 1;
                return Ok(());
            }
        };
        self.own_inbox(&watch)?.dirty = true;
        Ok(())
    }
    fn own_inbox(&mut self, watch: &str) -> Result<&mut WatchInbox, GatewayError> {
        self.inbox
            .as_mut()
            .filter(|inbox| inbox.watch == watch)
            .ok_or(GatewayError::Protocol)
    }
}

pub(crate) struct Session {
    socket: Option<Channel>,
    clock: Arc<dyn Clock>,
    policy: GatewayPolicy,
    next_request: u64,
    next_operation: u64,
    active: Option<GatewayOutcome>,
    ready: ProductSessionReady,
    events: Events,
}
impl Session {
    /// Open one protected product session at the gateway's native `address`.
    /// Every step, from the TCP connect to ready, shares one absolute
    /// handshake deadline.
    pub(crate) fn connect(
        address: SocketAddr,
        device: DeviceEvidence<'_>,
        client_id: &str,
        connector: &dyn GatewayConnector,
        clock: Arc<dyn Clock>,
        cancellation: Arc<dyn Cancellation>,
        policy: GatewayPolicy,
    ) -> Result<Self, GatewayError> {
        // Refuse oversized borrowed fields before allocating their owned DTO copies.
        for (value, ceiling) in [
            (device.credential, MAX_AUTH_CREDENTIAL_CHARACTERS),
            (client_id, MAX_PRODUCT_CLIENT_ID_CHARACTERS),
        ] {
            if value.chars().take(ceiling + 1).count() > ceiling {
                return Err(GatewayError::InvalidCredential);
            }
        }
        let deadline = clock
            .elapsed_ms()
            .checked_add(policy.handshake_ms())
            .ok_or(GatewayError::TimedOut)?;
        if cancellation.cancelled() {
            return Err(GatewayError::Cancelled);
        }
        let remaining = deadline
            .checked_sub(clock.elapsed_ms())
            .filter(|ms| *ms > 0)
            .ok_or(GatewayError::TimedOut)?;
        let stream = connector
            .connect(address, Duration::from_millis(remaining))
            .map_err(|error| io_cause(&error))?;
        let stream = DeadlineStream::new(stream, clock.clone(), cancellation, deadline);
        stream.remaining()?;
        let failure = stream.failure_owner();
        let transport =
            NativeTransport::connect(stream, device.identity, GatewayTrust::Pinned(device.pin))
                .map_err(|_| failure.take().unwrap_or(GatewayError::NativeHandshake))?;
        let mut selector = EnrollmentChannel::new(transport);
        let envelope = encode_envelope(&NativePairingRequest::OpenProduct)
            .map_err(|_| GatewayError::Protocol)?;
        selector
            .send_envelope(&envelope)
            .map_err(|_| failure.take().unwrap_or(GatewayError::Transport))?;
        let mut socket = Channel {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES),
        };
        let mut events = Events::new(policy);
        let mut message = read_opening_frame(&mut socket)?;
        let challenge = loop {
            match message {
                OutgoingMessage::Event(event)
                    if event.event == product_event::SESSION_CHALLENGE =>
                {
                    break shape_decode::<SessionChallenge>(
                        event.payload,
                        wire_shape_session_challenge,
                    )?;
                }
                OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
                OutgoingMessage::Event(event) => events.observe(event)?,
            }
            message = read_frame(&mut socket)?;
        };
        if !supports_product_version(challenge.min_version, challenge.max_version) {
            return Err(GatewayError::Protocol);
        }
        let params = SessionAuthenticateParams {
            min_version: PRODUCT_VERSION,
            max_version: PRODUCT_VERSION,
            nonce: challenge.nonce,
            credential: device.credential.to_owned(),
            client: ProductClientMetadata {
                id: client_id.to_owned(),
            },
        };
        if !wire_shape_session_authenticate_params(
            &serde_json::to_value(&params).map_err(|_| GatewayError::Protocol)?,
        ) {
            return Err(GatewayError::InvalidCredential);
        }
        let request_id = "0";
        send_request(&mut socket, request_id, PRODUCT_HANDSHAKE_METHOD, &params)?;
        let response = read_response(&mut socket, request_id, &mut events)?;
        let ready = shape_decode::<ProductSessionReady>(
            response_payload(response, RpcKind::Authentication)?,
            wire_shape_product_session_ready,
        )?;
        Ok(Self {
            socket: Some(socket),
            clock,
            policy,
            next_request: 1,
            next_operation: 1,
            active: None,
            ready,
            events: Events::new(policy),
        })
    }
    pub(crate) fn ready(&self) -> &ProductSessionReady {
        &self.ready
    }
    pub(crate) fn begin(&mut self) -> Result<u64, GatewayError> {
        if self.active.is_some() {
            return Err(GatewayError::Busy);
        }
        let operation = self.next_operation;
        self.next_operation = operation.checked_add(1).ok_or(GatewayError::Busy)?;
        let deadline = self
            .clock
            .elapsed_ms()
            .checked_add(self.policy.operation_ms())
            .ok_or_else(|| self.fail(GatewayError::TimedOut))?;
        let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
        socket.transport.stream_mut().begin_operation(deadline);
        if let Err(error) = socket.transport.stream_mut().remaining() {
            return Err(self.fail(error));
        }
        self.active = Some(GatewayOutcome {
            operation,
            failure: None,
        });
        self.events.unexpected = 0;
        Ok(operation)
    }
    pub(crate) fn finish(&mut self) -> Result<GatewayOutcome, GatewayError> {
        self.active.take().ok_or(GatewayError::Busy)
    }
    /// Fail the current operation with its first cause. The connection is
    /// closed unless `error` is a typed refusal the gateway answered on an
    /// intact connection that a later operation may ask again (row W17).
    pub(crate) fn fail(&mut self, error: GatewayError) -> GatewayError {
        let retained = self
            .active
            .as_mut()
            .map(|active| *active.failure.get_or_insert(error))
            .unwrap_or(error);
        if !keeps_connection(error) {
            self.socket.take();
        }
        retained
    }
    pub(super) fn rpc<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
        kind: RpcKind,
    ) -> Result<Value, GatewayError> {
        let active = self.active.as_ref().ok_or(GatewayError::Busy)?;
        if let Some(error) = active.failure {
            return Err(error);
        }
        let result = self.rpc_inner(method, params, kind);
        result.map_err(|error| self.fail(error))
    }
    /// Register this connection's one record watch inside the current
    /// operation. Its hints are kept from the acknowledgement on.
    pub(crate) fn watch_records(
        &mut self,
        params: &ConversationWatchRecordsParams,
    ) -> Result<ChangeWatchId, GatewayError> {
        let value = self.rpc(
            product_method::CONVERSATION_WATCH_RECORDS,
            params,
            RpcKind::Watch,
        )?;
        let result = strict_decode::<ConversationWatchResult>(value)
            .and_then(|result| {
                // The complete published identity contract, so the inbox and
                // the `registered` line hold only a conforming identity.
                conforming_watch_id(&result.watch_id)
                    .then_some(result)
                    .ok_or(GatewayError::Protocol)
            })
            .map_err(|error| self.fail(error))?;
        self.events.inbox = Some(WatchInbox {
            watch: result.watch_id.clone(),
            dirty: false,
            ended: None,
        });
        Ok(result.watch_id)
    }
    /// Wait, inside the current operation and under its deadline, until the
    /// watch has a hint or has ended. A hint already kept from an earlier
    /// operation returns at once; the watch's end comes before a hint.
    pub(crate) fn wait_hint(&mut self) -> Result<Wait, GatewayError> {
        let active = self.active.as_ref().ok_or(GatewayError::Busy)?;
        if let Some(error) = active.failure {
            return Err(error);
        }
        let result = self.wait_inner();
        result.map_err(|error| self.fail(error))
    }
    fn wait_inner(&mut self) -> Result<Wait, GatewayError> {
        let mut during_pass = true;
        loop {
            let inbox = self.events.inbox.as_mut().ok_or(GatewayError::Busy)?;
            if let Some(reason) = inbox.ended {
                return Ok(Wait::Ended(reason));
            }
            if std::mem::take(&mut inbox.dirty) {
                return Ok(Wait::Hint { during_pass });
            }
            during_pass = false;
            let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
            match read_frame(socket)? {
                // Nothing is pending while waiting (row W15).
                OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
                OutgoingMessage::Event(event) => self.events.observe(event)?,
            }
        }
    }
    fn rpc_inner<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
        kind: RpcKind,
    ) -> Result<Value, GatewayError> {
        let id = self.next_request.to_string();
        self.next_request = self.next_request.checked_add(1).ok_or(GatewayError::Busy)?;
        let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
        send_request(socket, &id, method, params)?;
        let response = read_response(socket, &id, &mut self.events)?;
        response_payload(response, kind)
    }
}
/// `source_preparing` is a complete, correlated answer: the gateway keeps its
/// preparation progress for the same read, so the connection stays usable for
/// the next operation. Every other failure leaves the stream's state unknown.
fn keeps_connection(error: GatewayError) -> bool {
    error == GatewayError::Record(RecordReadErrorCode::SourcePreparing)
}
fn shape_decode<T: DeserializeOwned>(
    value: Value,
    shape: fn(&Value) -> bool,
) -> Result<T, GatewayError> {
    if !shape(&value) {
        return Err(GatewayError::Protocol);
    }
    serde_json::from_value(value).map_err(|_| GatewayError::Protocol)
}
/// The generated watch DTOs refuse unknown fields; no other shape check exists.
fn strict_decode<T: DeserializeOwned>(value: Value) -> Result<T, GatewayError> {
    serde_json::from_value(value).map_err(|_| GatewayError::Protocol)
}
fn send_request<T: Serialize>(
    socket: &mut Channel,
    id: &str,
    method: &str,
    params: &T,
) -> Result<(), GatewayError> {
    let text = encode_request(id, method, params).map_err(|error| match error {
        ReadEncodeError::ResponseTooLarge => GatewayError::RequestTooLarge,
        ReadEncodeError::InvalidPayload => GatewayError::Protocol,
    })?;
    socket.send(&text)
}
fn read_response(
    socket: &mut Channel,
    id: &str,
    events: &mut Events,
) -> Result<ResponseFrame, GatewayError> {
    loop {
        match read_frame(socket)? {
            OutgoingMessage::Response(response) if response.id == id => return Ok(response),
            OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
            OutgoingMessage::Event(event) => events.observe(event)?,
        }
    }
}
/// The frame answering `openProduct`. A gateway that takes no product permit
/// for this connection answers the enrollment `Refused` reply on the same
/// framing (design rows PR2, PR10, PR13, PR15); otherwise product messages
/// begin. Only this frame is read through the enrollment reply decoder; every
/// later frame is a product message (`read_frame`), as
/// `open_product_refusal_is_typed_and_other_frames_stay_protocol` checks.
fn read_opening_frame(socket: &mut Channel) -> Result<OutgoingMessage, GatewayError> {
    let text = receive_text(socket)?;
    OutgoingMessage::decode(&text).map_err(|_| match decode_reply(text.as_bytes()) {
        Ok(NativePairingReply::Refused) => GatewayError::ProductRefused,
        _ => GatewayError::Protocol,
    })
}
fn read_frame(socket: &mut Channel) -> Result<OutgoingMessage, GatewayError> {
    let text = receive_text(socket)?;
    OutgoingMessage::decode(&text).map_err(|_| GatewayError::Protocol)
}
fn receive_text(socket: &mut Channel) -> Result<String, GatewayError> {
    // Buffered plaintext still consumes this same absolute operation deadline.
    socket.transport.stream_mut().remaining()?;
    let text = socket.receive()?;
    socket.transport.stream_mut().remaining()?;
    Ok(text)
}
fn response_payload(response: ResponseFrame, kind: RpcKind) -> Result<Value, GatewayError> {
    if response.ok {
        return response.payload.ok_or(GatewayError::Protocol);
    }
    let error = response.error.ok_or(GatewayError::Protocol)?;
    match kind {
        RpcKind::Record => serde_json::from_value::<RecordReadErrorCode>(Value::String(error.code))
            .map(GatewayError::Record)
            .map_or_else(|_| Err(GatewayError::Protocol), Err),
        RpcKind::Catalogue => {
            serde_json::from_value::<CatalogueReadErrorCode>(Value::String(error.code))
                .map(GatewayError::Catalogue)
                .map_or_else(|_| Err(GatewayError::Protocol), Err)
        }
        RpcKind::Watch => serde_json::from_value::<ChangeWatchErrorCode>(Value::String(error.code))
            .map(GatewayError::Watch)
            .map_or_else(|_| Err(GatewayError::Protocol), Err),
        RpcKind::Authentication => Err(GatewayError::Authentication(authentication_close_reason(
            &error.code,
        ))),
    }
}

/// Whether `id` is within the published change-watch identity contract: its
/// byte bound and its generated pattern.
fn conforming_watch_id(id: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    id.len() <= MAX_CHANGE_WATCH_ID_BYTES
        && PATTERN
            .get_or_init(|| {
                regex::Regex::new(CHANGE_WATCH_ID_PATTERN).expect("generated pattern compiles")
            })
            .is_match(id)
}
