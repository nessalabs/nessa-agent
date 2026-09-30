//! One socket and one attempt outcome. Facades never own their own connection.
use super::deadline_stream::{io_cause, DeadlineStream};
use crate::product_contract::generated::{
    CatalogueReadErrorCode, RecordReadErrorCode, SessionCloseReason,
};
use crate::{
    app::ports::Clock,
    product::{
        generated::{
            product_event, wire_shape_product_session_ready,
            wire_shape_session_authenticate_params, wire_shape_session_challenge,
            ProductClientMetadata, ProductSessionReady, SessionAuthenticateParams,
            SessionChallenge, MAX_AUTH_CREDENTIAL_CHARACTERS, MAX_PRODUCT_CLIENT_ID_CHARACTERS,
            MAX_RECORD_RESPONSE_BYTES, PRODUCT_HANDSHAKE_METHOD, PRODUCT_SESSION_PATH,
            PRODUCT_VERSION,
        },
        passive_read::wire::{encode_request, ReadEncodeError},
        wire::{authentication_close_reason, supports_product_version},
    },
    protocol::{OutgoingMessage, ResponseFrame},
    read_only_sync::application::{
        Cancellation, GatewayConnector, GatewayError, GatewayOutcome, GatewayPolicy, GatewayStream,
    },
};
use nessa_gateway_endpoint::domain::GatewayEndpoint;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tungstenite::{client, protocol::WebSocketConfig, Error, Message, WebSocket};

pub(crate) struct LocalConnector;
impl GatewayConnector for LocalConnector {
    fn connect(
        &self,
        address: SocketAddr,
        timeout: Duration,
    ) -> io::Result<Box<dyn GatewayStream>> {
        TcpStream::connect_timeout(&address, timeout)
            .map(|socket| Box::new(Socket(socket)) as Box<dyn GatewayStream>)
    }
}
struct Socket(TcpStream);
impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}
impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl GatewayStream for Socket {
    fn read_timeout(&self, timeout: Duration) -> io::Result<()> {
        self.0.set_read_timeout(Some(timeout))
    }
    fn write_timeout(&self, timeout: Duration) -> io::Result<()> {
        self.0.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) -> io::Result<()> {
        self.0.shutdown(Shutdown::Both)
    }
}

#[derive(Clone, Copy)]
pub(super) enum RpcKind {
    Authentication,
    Record,
    Catalogue,
}

pub(crate) struct Session {
    socket: Option<WebSocket<DeadlineStream>>,
    clock: Arc<dyn Clock>,
    policy: GatewayPolicy,
    next_request: u64,
    next_operation: u64,
    active: Option<GatewayOutcome>,
    ready: ProductSessionReady,
    events: usize,
    controls: usize,
}
impl Session {
    pub(crate) fn connect(
        endpoint: &GatewayEndpoint,
        credential: &str,
        client_id: &str,
        connector: &dyn GatewayConnector,
        clock: Arc<dyn Clock>,
        cancellation: Arc<dyn Cancellation>,
        policy: GatewayPolicy,
    ) -> Result<Self, GatewayError> {
        // Refuse oversized borrowed fields before allocating their owned DTO copies.
        for (value, ceiling) in [
            (credential, MAX_AUTH_CREDENTIAL_CHARACTERS),
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
            .connect(endpoint.socket_address(), Duration::from_millis(remaining))
            .map_err(|error| io_cause(&error))?;
        let stream = DeadlineStream::new(
            stream,
            clock.clone(),
            cancellation,
            deadline,
            policy.upgrade_bytes(),
        );
        stream.remaining()?;
        let failure = stream.failure_owner();
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_RECORD_RESPONSE_BYTES))
            .max_frame_size(Some(MAX_RECORD_RESPONSE_BYTES))
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_RECORD_RESPONSE_BYTES);
        let url = format!("{}{}", endpoint.web_socket_url(), PRODUCT_SESSION_PATH);
        let (mut socket, _) = client::client_with_config(url, stream, Some(config))
            .map_err(|_| failure.take().unwrap_or(GatewayError::Transport))?;
        socket.get_mut().finish_upgrade();
        let mut events = 0;
        let mut controls = 0;
        let challenge = loop {
            match read_frame(&mut socket, policy, &mut events, &mut controls)? {
                OutgoingMessage::Event(event)
                    if event.event == product_event::SESSION_CHALLENGE =>
                {
                    break shape_decode::<SessionChallenge>(
                        event.payload,
                        wire_shape_session_challenge,
                    )?;
                }
                OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
                _ => count_event(policy, &mut events)?,
            }
        };
        if !supports_product_version(challenge.min_version, challenge.max_version) {
            return Err(GatewayError::Protocol);
        }
        let params = SessionAuthenticateParams {
            min_version: PRODUCT_VERSION,
            max_version: PRODUCT_VERSION,
            nonce: challenge.nonce,
            credential: credential.to_owned(),
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
        let response = read_response(&mut socket, request_id, policy, &mut events, &mut controls)?;
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
            events: 0,
            controls: 0,
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
        socket.get_mut().begin_operation(deadline);
        if let Err(error) = socket.get_ref().remaining() {
            return Err(self.fail(error));
        }
        self.active = Some(GatewayOutcome {
            operation,
            failure: None,
        });
        self.events = 0;
        self.controls = 0;
        Ok(operation)
    }
    pub(crate) fn finish(&mut self) -> Result<GatewayOutcome, GatewayError> {
        self.active.take().ok_or(GatewayError::Busy)
    }
    pub(crate) fn fail(&mut self, error: GatewayError) -> GatewayError {
        let retained = self
            .active
            .as_mut()
            .map(|active| *active.failure.get_or_insert(error))
            .unwrap_or(error);
        self.socket.take();
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
        let response = read_response(
            socket,
            &id,
            self.policy,
            &mut self.events,
            &mut self.controls,
        )?;
        response_payload(response, kind)
    }
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
fn send_request<T: Serialize>(
    socket: &mut WebSocket<DeadlineStream>,
    id: &str,
    method: &str,
    params: &T,
) -> Result<(), GatewayError> {
    let text = encode_request(id, method, params).map_err(|error| match error {
        ReadEncodeError::ResponseTooLarge => GatewayError::RequestTooLarge,
        ReadEncodeError::InvalidPayload => GatewayError::Protocol,
    })?;
    socket
        .send(Message::Text(text.into()))
        .map_err(|error| socket_error(socket, error))
}
fn count_event(policy: GatewayPolicy, events: &mut usize) -> Result<(), GatewayError> {
    if *events >= policy.unexpected_events() {
        return Err(GatewayError::EventCapacity);
    }
    *events += 1;
    Ok(())
}
fn read_response(
    socket: &mut WebSocket<DeadlineStream>,
    id: &str,
    policy: GatewayPolicy,
    events: &mut usize,
    controls: &mut usize,
) -> Result<ResponseFrame, GatewayError> {
    loop {
        match read_frame(socket, policy, events, controls)? {
            OutgoingMessage::Response(response) if response.id == id => return Ok(response),
            OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
            OutgoingMessage::Event(_) => count_event(policy, events)?,
        }
    }
}
fn read_frame(
    socket: &mut WebSocket<DeadlineStream>,
    policy: GatewayPolicy,
    _events: &mut usize,
    controls: &mut usize,
) -> Result<OutgoingMessage, GatewayError> {
    loop {
        // Buffered frames still consume this same absolute operation deadline.
        socket.get_ref().remaining()?;
        let message = socket.read().map_err(|error| socket_error(socket, error))?;
        socket.get_ref().remaining()?;
        match message {
            Message::Text(text) => {
                return OutgoingMessage::decode(&text).map_err(|_| GatewayError::Protocol)
            }
            Message::Ping(_) | Message::Pong(_) => {
                if *controls >= policy.control_frames() {
                    return Err(GatewayError::ControlCapacity);
                }
                *controls += 1;
            }
            Message::Close(frame) => {
                return Err(GatewayError::Closed(frame.and_then(|frame| {
                    SessionCloseReason::from_web_socket_code(u16::from(frame.code))
                })))
            }
            _ => return Err(GatewayError::Protocol),
        }
    }
}
fn socket_error(socket: &mut WebSocket<DeadlineStream>, error: Error) -> GatewayError {
    socket
        .get_mut()
        .take_failure()
        .unwrap_or_else(|| match error {
            Error::Io(error) => io_cause(&error),
            Error::Capacity(_) => GatewayError::ResponseTooLarge,
            Error::ConnectionClosed | Error::AlreadyClosed => GatewayError::Closed(None),
            _ => GatewayError::Protocol,
        })
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
        RpcKind::Authentication => Err(GatewayError::Authentication(authentication_close_reason(
            &error.code,
        ))),
    }
}
