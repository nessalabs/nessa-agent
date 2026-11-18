//! One protected native connection and one attempt outcome. Facades never own
//! their own connection.
//!
//! The device dials the gateway's native address, proves its key over TLS with
//! the gateway key strictly pinned, selects the product session with a first
//! `openProduct` envelope, and authenticates with its issued credential id.
//! Product messages then travel one per length-prefixed frame (design
//! "Protected reads over the native channel", rows PR1, PR3, PR9).
//!
//! A session may hold one `conversation.subscribe` feed. Its view and end
//! frames are kept whenever they arrive, so an operation waiting for its own
//! response never counts them as unexpected events. That includes a view that
//! arrives in the same turn as the subscribe reply, before that reply has
//! named the subscription.
use crate::protected::{io_cause, Channel, DeadlineStream};
use crate::read_only_sync::application::{
    follow::{Ended, SubscriptionEnd, ViewCursor, Wait},
    held_file::busy,
    Cancellation, GatewayConnector, GatewayError, GatewayOutcome, GatewayPolicy, GatewayStream,
    HeldFileRefusal, SubscriptionCode, SubscriptionRefusal,
};
use nessa_auth::adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::{
    wire::{
        decode_reply, encode_request as encode_envelope, NativePairingReply, NativePairingRequest,
    },
    EnrollmentChannel, FrameReader, MAX_PROTECTED_RESPONSE_BYTES,
};
use nessa_protocol::product::generated::{
    product_event, product_method, wire_shape_product_session_ready,
    wire_shape_session_authenticate_params, wire_shape_session_challenge,
    ConversationSubscribeParams, ConversationSubscribeResult, ConversationSubscriptionEndReason,
    ConversationSubscriptionEnded, ConversationUnsubscribeParams, ConversationViewCursor,
    ConversationViewed, GrantsChanged, GrantsSubscribeParams, ProductClientMetadata,
    ProductSessionReady, ProductSurface, ProductSurfaceKind, SessionAuthenticateParams,
    SessionChallenge, CONVERSATION_SUBSCRIPTION_ID_PATTERN, MAX_AUTH_CREDENTIAL_CHARACTERS,
    MAX_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS, MAX_PRODUCT_CLIENT_ID_CHARACTERS,
    MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS, MIN_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS,
    PRODUCT_HANDSHAKE_METHOD, PRODUCT_VERSION, TRANSCRIPT_STATES,
};
use nessa_protocol::product::handshake::{
    accepted_termination, authentication_close_reason, supports_product_version,
};
use nessa_protocol::product::passive_read::{
    conforming_view_cursor, encode_request, view_cursor_behind, ReadEncodeError,
};
use nessa_protocol::product_contract::generated::{
    CatalogueReadErrorCode, ConversationErrorCode, RecordReadErrorCode,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage, ResponseFrame};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::io::{Read, Result as IoResult, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
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

#[derive(Clone, Copy)]
pub(super) enum RpcKind {
    Authentication,
    Record,
    Catalogue,
    HeldFile,
    Subscribe,
}

/// Views kept until the caller takes them. Each one replaces what the caller
/// shows, so a burst past [`QUEUED_VIEWS`] drops the oldest and keeps the
/// newest in arrival order.
const QUEUED_VIEWS: usize = 4;
/// Grant frames kept while a record read shares this connection. A further
/// frame closes it. Dropping one would skip an incremental page; the follow
/// retries from the cursor it has committed.
const QUEUED_GRANT_FRAMES: usize = 8;
/// Bytes of those queued frames, measured as the event payload's JSON.
const QUEUED_GRANT_BYTES: usize = 512 * 1024;

struct Parked {
    views: Vec<ConversationViewed>,
    ended: Option<ConversationSubscriptionEnded>,
}
impl Parked {
    fn push_view(&mut self, viewed: ConversationViewed) {
        if self.ended.is_some() {
            return;
        }
        if self.views.len() == QUEUED_VIEWS {
            self.views.remove(0);
            tracing::warn!(
                limit = "client.subscription_views",
                "product session hit an operational limit"
            );
        }
        self.views.push(viewed);
    }
}

/// The one subscription this connection opened. `closed` is set once its end
/// has been delivered, so a view that arrives after that end is ignored.
/// `conversation` is the id this connection subscribed to. A view for another
/// conversation is rejected before it is queued.
struct SubscriptionInbox {
    id: String,
    conversation: String,
    parked: Parked,
    closed: bool,
}

/// One grant-subscription delivery.
pub(crate) enum GrantDelivery {
    Changed(GrantsChanged),
    Ended(ConversationSubscriptionEnded),
}

/// One delivery from the subscription feed, in arrival order.
pub(crate) enum SubscriptionDelivery {
    View(Box<ConversationViewed>),
    Ended(ConversationSubscriptionEnded),
}

/// How events are read for one session: a subscription's views and end go to
/// its inbox. A view that
/// arrives before the subscribe reply is parked on `opening` and taken when
/// that reply names the subscription. Every other event counts toward the
/// operation's capacity.
struct Events {
    policy: GatewayPolicy,
    unexpected: usize,
    subscription: Option<SubscriptionInbox>,
    /// Set while `conversation.subscribe` is unanswered.
    opening: Option<Parked>,
    /// Conversation requested by the subscribe in flight, until the inbox
    /// takes it. A view for any other conversation is a protocol failure.
    opening_conversation: Option<String>,
    /// No kept view in this incarnation may sit behind this cursor. It starts
    /// as `conversation.subscribe`'s `after` and moves to each view kept.
    floor: Option<ConversationViewCursor>,
    /// Grant frames for this connection's one `grants.subscribe`, with the
    /// payload bytes counted toward [`QUEUED_GRANT_BYTES`].
    grants: Vec<(usize, GrantsChanged)>,
    grant_bytes: usize,
    grant_ended: Option<ConversationSubscriptionEnded>,
    grant_id: Option<String>,
    /// Set while `grants.subscribe` is unanswered, so a frame that arrives
    /// with the reply is kept.
    grant_opening: bool,
}
impl Events {
    fn new(policy: GatewayPolicy) -> Self {
        Self {
            policy,
            unexpected: 0,
            subscription: None,
            opening: None,
            opening_conversation: None,
            floor: None,
            grants: Vec::new(),
            grant_bytes: 0,
            grant_ended: None,
            grant_id: None,
            grant_opening: false,
        }
    }
    /// `session.terminated` is this connection's close, the same acceptance
    /// the product worker uses. It is not queued and it does not count as
    /// unexpected. A malformed termination is a protocol failure.
    fn observe(&mut self, event: EventFrame) -> Result<(), GatewayError> {
        match event.event.as_str() {
            product_event::SESSION_TERMINATED => match accepted_termination(&event.payload) {
                Some(termination) => Err(GatewayError::Closed(Some(termination.code))),
                None => Err(GatewayError::Protocol),
            },
            product_event::CONVERSATION_VIEW => self.observe_view(event.payload),
            product_event::CONVERSATION_SUBSCRIPTION_ENDED => {
                self.observe_subscription_end(event.payload)
            }
            product_event::GRANTS_CHANGED => self.observe_grants(event.payload),
            product_event::GRANTS_SUBSCRIPTION_ENDED => self.observe_grant_end(event.payload),
            _ => self.count_unexpected(),
        }
    }
    /// A view outside the generated shape, including a cursor this client
    /// could not pass back to `follow` or a `transcriptState` outside the
    /// published set, is skipped. One behind the resume floor in the same
    /// incarnation is skipped. One for another subscription than the one this
    /// connection opened, or for a different conversation than the one it
    /// subscribed to, is a protocol failure.
    fn observe_view(&mut self, payload: serde_json::Value) -> Result<(), GatewayError> {
        let viewed = match strict_decode::<ConversationViewed>(payload) {
            Ok(viewed)
                if conforming_subscription_id(&viewed.subscription_id)
                    && conforming_view_cursor(&viewed.cursor)
                    && published_transcript_state(&viewed.view.transcript_state) =>
            {
                viewed
            }
            _ => {
                tracing::warn!(
                    limit = "client.subscription_view",
                    "skipped a conversation view outside the generated shape"
                );
                return Ok(());
            }
        };
        let expected = self
            .subscription
            .as_ref()
            .map(|inbox| inbox.conversation.as_str())
            .or(self.opening_conversation.as_deref());
        if expected.is_some_and(|conversation| viewed.view.conversation_id != conversation) {
            return Err(GatewayError::Protocol);
        }
        if let Some(live) = &self.subscription {
            if viewed.subscription_id != live.id {
                return Err(GatewayError::Protocol);
            }
            if live.closed {
                return Ok(());
            }
        }
        if (self.subscription.is_some() || self.opening.is_some()) && !self.keep_at_floor(&viewed) {
            return Ok(());
        }
        if let Some(live) = &mut self.subscription {
            live.parked.push_view(viewed);
            return Ok(());
        }
        if let Some(opening) = &mut self.opening {
            opening.push_view(viewed);
            return Ok(());
        }
        self.count_unexpected()
    }
    /// A view behind the floor in the same incarnation is not queued, and
    /// does not move the floor. An equal position and another incarnation are
    /// kept, and the floor becomes the cursor just kept.
    fn keep_at_floor(&mut self, viewed: &ConversationViewed) -> bool {
        if self
            .floor
            .as_ref()
            .is_some_and(|floor| view_cursor_behind(&viewed.cursor, floor))
        {
            tracing::warn!(
                limit = "client.subscription_view",
                "skipped a conversation view behind the resume cursor"
            );
            return false;
        }
        self.floor = Some(owned_cursor(&viewed.cursor));
        true
    }
    fn observe_subscription_end(&mut self, payload: serde_json::Value) -> Result<(), GatewayError> {
        let mut ended = match strict_decode::<ConversationSubscriptionEnded>(payload) {
            Ok(ended) if conforming_subscription_id(&ended.subscription_id) => ended,
            _ => {
                tracing::warn!(
                    limit = "client.subscription_end",
                    "skipped a subscription end outside the generated shape"
                );
                return Ok(());
            }
        };
        // A resume cursor follow would reject is not offered. The end still
        // arrives, so the caller subscribes again without it.
        if ended
            .last_delivered
            .as_ref()
            .is_some_and(|cursor| !conforming_view_cursor(cursor))
        {
            tracing::warn!(
                limit = "client.subscription_end",
                "dropped a resume cursor outside the published view cursor"
            );
            ended.last_delivered = None;
        }
        let parked = if let Some(live) = &mut self.subscription {
            if ended.subscription_id != live.id {
                return Err(GatewayError::Protocol);
            }
            if live.closed {
                return Ok(());
            }
            &mut live.parked
        } else if let Some(opening) = &mut self.opening {
            opening
        } else {
            return self.count_unexpected();
        };
        parked.ended.get_or_insert(ended);
        Ok(())
    }
    fn observe_grants(&mut self, payload: serde_json::Value) -> Result<(), GatewayError> {
        let changed = match strict_decode::<GrantsChanged>(payload) {
            Ok(changed) if conforming_subscription_id(&changed.subscription_id) => changed,
            _ => return Ok(()),
        };
        if let Some(id) = &self.grant_id {
            if changed.subscription_id != *id {
                return Err(GatewayError::Protocol);
            }
        } else if !self.grant_opening {
            return self.count_unexpected();
        }
        let bytes = serde_json::to_string(&changed)
            .map(|text| text.len())
            .unwrap_or(usize::MAX);
        if self.grants.len() >= QUEUED_GRANT_FRAMES
            || self.grant_bytes.saturating_add(bytes) > QUEUED_GRANT_BYTES
        {
            tracing::warn!(
                limit = "client.grant_frames",
                "product session hit an operational limit"
            );
            return Err(GatewayError::Protocol);
        }
        self.grant_bytes = self.grant_bytes.saturating_add(bytes);
        self.grants.push((bytes, changed));
        Ok(())
    }
    fn pop_grant(&mut self) -> Option<GrantsChanged> {
        if self.grants.is_empty() {
            return None;
        }
        let (bytes, changed) = self.grants.remove(0);
        self.grant_bytes = self.grant_bytes.saturating_sub(bytes);
        Some(changed)
    }
    fn observe_grant_end(&mut self, payload: serde_json::Value) -> Result<(), GatewayError> {
        let ended = match strict_decode::<ConversationSubscriptionEnded>(payload) {
            Ok(ended) if conforming_subscription_id(&ended.subscription_id) => ended,
            _ => return Ok(()),
        };
        if let Some(id) = &self.grant_id {
            if ended.subscription_id != *id {
                return Err(GatewayError::Protocol);
            }
        } else if !self.grant_opening {
            return self.count_unexpected();
        }
        self.grant_ended.get_or_insert(ended);
        Ok(())
    }
    fn count_unexpected(&mut self) -> Result<(), GatewayError> {
        if self.unexpected >= self.policy.unexpected_events() {
            return Err(GatewayError::EventCapacity);
        }
        self.unexpected += 1;
        Ok(())
    }
}

/// One `conversation.send` or `conversation.receipt` exchange.
/// A typed refusal leaves the connection up. Transport and protocol failures
/// are [`Lost`](Self::Lost) and the connection is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandExchange {
    /// The call did not finish, or the connection was closed.
    Lost(GatewayError),
    /// The gateway refused with this code. The connection stays up.
    Refused(String),
}
enum CommandInner {
    Transport(GatewayError),
    Refused(String),
    Protocol,
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
        Self::connect_surface(
            address,
            device,
            client_id,
            ProductSurfaceKind::Cli,
            client_id,
            connector,
            clock,
            cancellation,
            policy,
        )
    }
    /// The shared canonical handshake with caller-owned public surface metadata.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn connect_surface(
        address: SocketAddr,
        device: DeviceEvidence<'_>,
        client_id: &str,
        surface_kind: ProductSurfaceKind,
        instance: &str,
        connector: &dyn GatewayConnector,
        clock: Arc<dyn Clock>,
        cancellation: Arc<dyn Cancellation>,
        policy: GatewayPolicy,
    ) -> Result<Self, GatewayError> {
        // Refuse oversized borrowed fields before allocating owned DTO copies.
        for (value, ceiling) in [
            (device.credential, MAX_AUTH_CREDENTIAL_CHARACTERS),
            (client_id, MAX_PRODUCT_CLIENT_ID_CHARACTERS),
            (instance, MAX_PRODUCT_SURFACE_INSTANCE_CHARACTERS),
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
        // The run's own cancellation time bounds the connect too.
        let cut = cancellation
            .until_ms()
            .map(|until| until.saturating_sub(clock.elapsed_ms()))
            .filter(|left| *left < remaining);
        if cut == Some(0) {
            return Err(GatewayError::Cancelled);
        }
        let stream = connector
            .connect(address, Duration::from_millis(cut.unwrap_or(remaining)))
            .map_err(|error| match io_cause(&error) {
                GatewayError::TimedOut if cut.is_some() => GatewayError::Cancelled,
                cause => cause,
            })?;
        let observer = cancellation.clone();
        let stream = DeadlineStream::new(stream, clock.clone(), cancellation, deadline);
        stream.remaining()?;
        let failure = stream.failure_owner();
        let transport = NativeTransport::connect_observed(
            stream,
            device.identity,
            GatewayTrust::Pinned(device.pin),
            |_| observer.record_failure(failure.get().unwrap_or(GatewayError::NativeHandshake)),
        )
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
        // Keep the physical channel outside the result/panic scope. Publish a
        // known typed opening refusal before its socket cleanup can block.
        let opened = catch_unwind(AssertUnwindSafe(|| {
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
                surface: ProductSurface {
                    kind: surface_kind,
                    instance: instance.to_owned(),
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
            if ready.credential_id.as_bytes() != device.credential.as_bytes() {
                return Err(GatewayError::Correlation);
            }
            Ok(ready)
        }))
        .unwrap_or(Err(GatewayError::DriverPanicked));
        let ready = match opened {
            Ok(ready) => ready,
            Err(error) => {
                socket.transport.stream_mut().record_failure(error);
                return Err(error);
            }
        };
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
    pub(crate) fn into_product_channel(mut self) -> (Channel, ProductSessionReady) {
        (self.socket.take().expect("connected channel"), self.ready)
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
    /// Drop the socket. A typed unsubscribe refusal keeps the connection so a
    /// later operation can ask again; a feed that will not ask again drops it
    /// here, and the server ends the subscription with the connection.
    pub(crate) fn disconnect(&mut self) {
        self.socket.take();
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
    /// Open this connection's conversation subscription. A view that arrived
    /// with the reply is kept and comes out of [`Self::next_delivery`] first.
    pub(crate) fn subscribe(
        &mut self,
        params: &ConversationSubscribeParams,
    ) -> Result<String, GatewayError> {
        // One inbox per connection. A second subscribe would drop the first
        // id, and this client could no longer unsubscribe it.
        if self.events.subscription.is_some() {
            return Err(self.fail(GatewayError::Busy));
        }
        self.events.opening_conversation = Some(params.conversation_id.clone());
        self.events.floor = params.after.as_ref().map(owned_cursor);
        self.events.opening = Some(Parked {
            views: Vec::new(),
            ended: None,
        });
        let value = match self.rpc(
            product_method::CONVERSATION_SUBSCRIBE,
            params,
            RpcKind::Subscribe,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.events.opening = None;
                self.events.opening_conversation = None;
                self.events.floor = None;
                return Err(error);
            }
        };
        let result = match strict_decode::<ConversationSubscribeResult>(value) {
            Ok(result) if conforming_subscription_id(&result.subscription_id) => result,
            _ => {
                self.events.opening = None;
                self.events.opening_conversation = None;
                self.events.floor = None;
                return Err(self.fail(GatewayError::Protocol));
            }
        };
        let parked = self.events.opening.take().unwrap_or(Parked {
            views: Vec::new(),
            ended: None,
        });
        let Some(conversation) = self.events.opening_conversation.take() else {
            return Err(self.fail(GatewayError::Protocol));
        };
        let foreign = parked
            .views
            .iter()
            .any(|view| view.subscription_id != result.subscription_id)
            || parked
                .ended
                .as_ref()
                .is_some_and(|ended| ended.subscription_id != result.subscription_id);
        if foreign {
            return Err(self.fail(GatewayError::Protocol));
        }
        self.events.subscription = Some(SubscriptionInbox {
            id: result.subscription_id.clone(),
            conversation,
            parked,
            closed: false,
        });
        Ok(result.subscription_id)
    }
    /// Open this connection's grant subscription. A frame that arrived with
    /// the reply is kept and comes out of [`Self::next_grant`] first.
    pub(crate) fn subscribe_grants(
        &mut self,
        params: &GrantsSubscribeParams,
    ) -> Result<String, GatewayError> {
        if self.events.grant_id.is_some() || self.events.grant_opening {
            return Err(self.fail(GatewayError::Busy));
        }
        self.events.grant_opening = true;
        let value = match self.rpc(product_method::GRANTS_SUBSCRIBE, params, RpcKind::Subscribe) {
            Ok(value) => value,
            Err(error) => {
                self.events.grant_opening = false;
                self.events.grants.clear();
                self.events.grant_bytes = 0;
                self.events.grant_ended = None;
                return Err(error);
            }
        };
        let result = match strict_decode::<ConversationSubscribeResult>(value) {
            Ok(result) if conforming_subscription_id(&result.subscription_id) => result,
            _ => {
                self.events.grant_opening = false;
                return Err(self.fail(GatewayError::Protocol));
            }
        };
        self.events.grant_opening = false;
        self.events.grant_id = Some(result.subscription_id.clone());
        Ok(result.subscription_id)
    }
    pub(crate) fn grant_subscription(&self) -> Option<String> {
        self.events.grant_id.clone()
    }
    pub(crate) fn grant_page(
        &mut self,
        params: &nessa_protocol::product::generated::GrantsPageParams,
    ) -> Result<(), GatewayError> {
        let value = self.rpc(product_method::GRANTS_PAGE, params, RpcKind::Subscribe)?;
        let result = strict_decode::<ConversationSubscribeResult>(value)?;
        if result.subscription_id != params.subscription_id {
            return Err(self.fail(GatewayError::Protocol));
        }
        Ok(())
    }
    /// The next grant frame already queued, or one frame of socket progress.
    /// An idle tick is `Ok(None)` and leaves the socket up. A timeout of a
    /// blocking read is not used here: that path closes the connection.
    pub(crate) fn poll_grant(&mut self) -> Result<Option<GrantDelivery>, GatewayError> {
        if let Some(error) = self.active.as_ref().and_then(|active| active.failure) {
            return Err(error);
        }
        loop {
            if let Some(delivery) = self.take_grant()? {
                return Ok(Some(delivery));
            }
            if !self.poll_socket(200)? {
                return Ok(None);
            }
        }
    }
    fn take_grant(&mut self) -> Result<Option<GrantDelivery>, GatewayError> {
        if self.events.grant_id.is_none() {
            return Err(GatewayError::Busy);
        }
        if let Some(changed) = self.events.pop_grant() {
            return Ok(Some(GrantDelivery::Changed(changed)));
        }
        if let Some(ended) = self.events.grant_ended.take() {
            self.events.grant_id = None;
            return Ok(Some(GrantDelivery::Ended(ended)));
        }
        Ok(None)
    }
    /// The next view already queued, or one frame of socket progress. An idle
    /// tick is `Ok(None)` and leaves the socket up.
    pub(crate) fn poll_view(&mut self) -> Result<Option<Wait>, GatewayError> {
        if let Some(error) = self.active.as_ref().and_then(|active| active.failure) {
            return Err(error);
        }
        loop {
            if let Some(wait) = self.take_view()? {
                return Ok(Some(wait));
            }
            if !self.poll_socket(50)? {
                return Ok(None);
            }
        }
    }
    fn take_view(&mut self) -> Result<Option<Wait>, GatewayError> {
        let inbox = self
            .events
            .subscription
            .as_mut()
            .ok_or(GatewayError::Busy)?;
        if inbox.closed {
            return Err(GatewayError::Closed(None));
        }
        if let Some(ended) = inbox.parked.ended.take() {
            inbox.closed = true;
            inbox.parked.views.clear();
            let reason = match ended.reason {
                ConversationSubscriptionEndReason::Lagging => SubscriptionEnd::Lagging,
                ConversationSubscriptionEndReason::Refused => SubscriptionEnd::Refused,
                ConversationSubscriptionEndReason::SourceClosed => SubscriptionEnd::SourceClosed,
                ConversationSubscriptionEndReason::TooLarge => SubscriptionEnd::TooLarge,
            };
            let cause = (reason == SubscriptionEnd::Refused).then(|| {
                ended
                    .code
                    .map_or(GatewayError::Protocol, subscription_refusal)
            });
            return Ok(Some(Wait::Ended(Ended { reason, cause })));
        }
        if let Some(viewed) = inbox.parked.views.pop() {
            inbox.parked.views.clear();
            return Ok(Some(Wait::View {
                cursor: ViewCursor {
                    incarnation: viewed.cursor.incarnation,
                    position: viewed.cursor.position,
                },
                during_pass: true,
            }));
        }
        Ok(None)
    }
    /// Read one product frame without treating an idle tick as a failed
    /// operation. `true` when a frame was observed.
    fn poll_socket(&mut self, slice_ms: u64) -> Result<bool, GatewayError> {
        let polled = {
            let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
            socket
                .transport
                .stream_mut()
                .begin_poll(None, slice_ms, 5_000);
            socket.poll()
        };
        let text = match polled {
            Ok(text) => text,
            Err(error) => return Err(self.fail(error)),
        };
        let Some(text) = text else {
            return Ok(false);
        };
        let message =
            OutgoingMessage::decode(&text).map_err(|_| self.fail(GatewayError::Protocol))?;
        match message {
            OutgoingMessage::Response(_) => Err(self.fail(GatewayError::Correlation)),
            OutgoingMessage::Event(event) => {
                self.events
                    .observe(event)
                    .map_err(|error| self.fail(error))?;
                Ok(true)
            }
        }
    }
    /// The next kept view, or the subscription's end once every kept view
    /// has been taken. Waits under the current operation's deadline.
    pub(crate) fn next_delivery(&mut self) -> Result<SubscriptionDelivery, GatewayError> {
        let active = self.active.as_ref().ok_or(GatewayError::Busy)?;
        if let Some(error) = active.failure {
            return Err(error);
        }
        let result = self.delivery_inner();
        result.map_err(|error| self.fail(error))
    }
    fn delivery_inner(&mut self) -> Result<SubscriptionDelivery, GatewayError> {
        loop {
            let inbox = self
                .events
                .subscription
                .as_mut()
                .ok_or(GatewayError::Busy)?;
            if inbox.closed {
                return Err(GatewayError::Closed(None));
            }
            if !inbox.parked.views.is_empty() {
                return Ok(SubscriptionDelivery::View(Box::new(
                    inbox.parked.views.remove(0),
                )));
            }
            if let Some(ended) = inbox.parked.ended.take() {
                inbox.closed = true;
                return Ok(SubscriptionDelivery::Ended(ended));
            }
            let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
            match read_frame(socket)? {
                OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
                OutgoingMessage::Event(event) => self.events.observe(event)?,
            }
        }
    }
    /// Wait, inside the current operation and under its deadline, for the
    /// subscription's newest view or its end. A view kept from an earlier
    /// operation returns at once with `during_pass`; the older kept views are
    /// dropped, because each view replaces the one before it. The end comes
    /// before a view, as the subscription delivers nothing after it.
    pub(crate) fn wait_view(&mut self) -> Result<Wait, GatewayError> {
        let active = self.active.as_ref().ok_or(GatewayError::Busy)?;
        if let Some(error) = active.failure {
            return Err(error);
        }
        let result = self.wait_view_inner();
        result.map_err(|error| self.fail(error))
    }
    fn wait_view_inner(&mut self) -> Result<Wait, GatewayError> {
        let mut during_pass = true;
        loop {
            let inbox = self
                .events
                .subscription
                .as_mut()
                .ok_or(GatewayError::Busy)?;
            if inbox.closed {
                return Err(GatewayError::Closed(None));
            }
            if let Some(ended) = inbox.parked.ended.take() {
                inbox.closed = true;
                inbox.parked.views.clear();
                let reason = match ended.reason {
                    ConversationSubscriptionEndReason::Lagging => SubscriptionEnd::Lagging,
                    ConversationSubscriptionEndReason::Refused => SubscriptionEnd::Refused,
                    ConversationSubscriptionEndReason::SourceClosed => {
                        SubscriptionEnd::SourceClosed
                    }
                    ConversationSubscriptionEndReason::TooLarge => SubscriptionEnd::TooLarge,
                };
                let cause = (reason == SubscriptionEnd::Refused).then(|| {
                    ended
                        .code
                        .map_or(GatewayError::Protocol, subscription_refusal)
                });
                return Ok(Wait::Ended(Ended { reason, cause }));
            }
            if let Some(viewed) = inbox.parked.views.pop() {
                inbox.parked.views.clear();
                return Ok(Wait::View {
                    cursor: ViewCursor {
                        incarnation: viewed.cursor.incarnation,
                        position: viewed.cursor.position,
                    },
                    during_pass,
                });
            }
            during_pass = false;
            let socket = self.socket.as_mut().ok_or(GatewayError::Transport)?;
            match read_frame(socket)? {
                // Nothing is pending while waiting.
                OutgoingMessage::Response(_) => return Err(GatewayError::Correlation),
                OutgoingMessage::Event(event) => self.events.observe(event)?,
            }
        }
    }
    /// Stop the subscription this connection opened. A frame already queued
    /// is dropped with it: the caller asked to stop.
    pub(crate) fn unsubscribe(&mut self) -> Result<(), GatewayError> {
        let id = self
            .events
            .subscription
            .as_ref()
            .map(|inbox| inbox.id.clone())
            .ok_or(GatewayError::Busy)?;
        let value = self.rpc(
            product_method::CONVERSATION_UNSUBSCRIBE,
            &ConversationUnsubscribeParams {
                subscription_id: id.clone(),
            },
            RpcKind::Subscribe,
        )?;
        let result = strict_decode::<ConversationSubscribeResult>(value)
            .map_err(|error| self.fail(error))?;
        if result.subscription_id != id {
            return Err(self.fail(GatewayError::Protocol));
        }
        self.events.subscription = None;
        self.events.opening_conversation = None;
        Ok(())
    }
    /// One `conversation.send` or `conversation.receipt` inside the current
    /// operation. A typed refusal leaves the connection up. A transport or
    /// protocol failure closes it, as other product calls do.
    pub(crate) fn product_command<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
    ) -> Result<Value, CommandExchange> {
        let active = match self.active.as_ref() {
            Some(active) => active,
            None => return Err(CommandExchange::Lost(GatewayError::Busy)),
        };
        if let Some(error) = active.failure {
            return Err(CommandExchange::Lost(error));
        }
        match self.command_inner(method, params) {
            Ok(value) => Ok(value),
            Err(CommandInner::Transport(error)) => Err(CommandExchange::Lost(self.fail(error))),
            Err(CommandInner::Refused(code)) => Err(CommandExchange::Refused(code)),
            Err(CommandInner::Protocol) => {
                Err(CommandExchange::Lost(self.fail(GatewayError::Protocol)))
            }
        }
    }
    fn command_inner<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
    ) -> Result<Value, CommandInner> {
        let id = self.next_request.to_string();
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(CommandInner::Transport(GatewayError::Busy))?;
        let socket = self
            .socket
            .as_mut()
            .ok_or(CommandInner::Transport(GatewayError::Transport))?;
        send_request(socket, &id, method, params).map_err(CommandInner::Transport)?;
        let response =
            read_response(socket, &id, &mut self.events).map_err(CommandInner::Transport)?;
        if response.ok {
            return response.payload.ok_or(CommandInner::Protocol);
        }
        let error = response.error.ok_or(CommandInner::Protocol)?;
        if error.code.is_empty() {
            return Err(CommandInner::Protocol);
        }
        Err(CommandInner::Refused(error.code))
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
/// the next operation. `wrong_owner` is one too: the gateway answered that
/// this reader holds no grant on that one conversation, which says nothing
/// about the next read on another. A busy `artifact.read` range is one as
/// well: the same range is asked again on this connection. Every other
/// failure leaves the stream's state unknown.
fn keeps_connection(error: GatewayError) -> bool {
    matches!(
        error,
        GatewayError::Record(
            RecordReadErrorCode::SourcePreparing | RecordReadErrorCode::WrongOwner
        ) | GatewayError::Subscription(_)
    ) || busy(error)
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
/// The generated DTOs refuse unknown fields; no other shape check exists.
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
        RpcKind::HeldFile => Err(held_file_refusal(error.code)),
        RpcKind::Subscribe => Err(subscription_refusal(error.code)),
        RpcKind::Authentication => Err(GatewayError::Authentication(authentication_close_reason(
            &error.code,
        ))),
    }
}
pub(crate) fn subscription_refusal(code: String) -> GatewayError {
    let refusal = match code.as_str() {
        "forbidden" => SubscriptionRefusal::Forbidden,
        "unauthorized" => SubscriptionRefusal::Unauthorized,
        "subscription_capacity" => SubscriptionRefusal::Subscription(SubscriptionCode::Capacity),
        "subscription_duplicate" => SubscriptionRefusal::Subscription(SubscriptionCode::Duplicate),
        "unknown_subscription" => SubscriptionRefusal::Subscription(SubscriptionCode::Unknown),
        "cursor_ahead" | "incarnation_changed" | "grant_log_floor" => {
            SubscriptionRefusal::Subscription(SubscriptionCode::CursorAhead)
        }
        _ => match serde_json::from_value::<ConversationErrorCode>(serde_json::Value::String(code))
        {
            Ok(code) => SubscriptionRefusal::Conversation(code),
            Err(_) => return GatewayError::Protocol,
        },
    };
    GatewayError::Subscription(refusal)
}
/// An `artifact.read` refusal: a conversation code, or the socket's access
/// refusals. A code this client was not taught is a protocol failure, never
/// passed on as if known.
fn held_file_refusal(code: String) -> GatewayError {
    let refusal = match code.as_str() {
        "forbidden" => HeldFileRefusal::Forbidden,
        "unauthorized" => HeldFileRefusal::Unauthorized,
        code if code == ConversationErrorCode::UnknownMethod.as_str() => {
            HeldFileRefusal::UnknownMethod
        }
        _ => match serde_json::from_value::<ConversationErrorCode>(Value::String(code)) {
            Ok(code) => HeldFileRefusal::Conversation(code),
            Err(_) => return GatewayError::Protocol,
        },
    };
    GatewayError::HeldFile(refusal)
}

/// Whether `state` is one of the published `transcriptState` values.
fn published_transcript_state(state: &str) -> bool {
    TRANSCRIPT_STATES.contains(&state)
}

fn owned_cursor(cursor: &ConversationViewCursor) -> ConversationViewCursor {
    ConversationViewCursor {
        incarnation: cursor.incarnation.clone(),
        position: cursor.position.clone(),
    }
}

/// Whether `id` is within the published subscription identity: its character
/// bound and [`CONVERSATION_SUBSCRIPTION_ID_PATTERN`].
fn conforming_subscription_id(id: &str) -> bool {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let length = id.chars().count();
    (MIN_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS..=MAX_CONVERSATION_SUBSCRIPTION_ID_CHARACTERS)
        .contains(&length)
        && PATTERN
            .get_or_init(|| {
                regex::Regex::new(CONVERSATION_SUBSCRIPTION_ID_PATTERN)
                    .expect("generated pattern compiles")
            })
            .is_match(id)
}

#[cfg(test)]
mod grant_inbox_tests {
    use super::{Events, QUEUED_GRANT_BYTES, QUEUED_GRANT_FRAMES};
    use crate::read_only_sync::application::GatewayPolicy;
    use nessa_protocol::product::generated::{product_event, GrantStreamCursor, GrantsChanged};
    use nessa_protocol::protocol::EventFrame;
    fn events() -> Events {
        let mut events = Events::new(GatewayPolicy::new(1_000, 1_000, 4).unwrap());
        events.grant_opening = true;
        events
    }

    fn frame(id: &str, marker: &str) -> GrantsChanged {
        GrantsChanged {
            subscription_id: id.to_owned(),
            cursor: GrantStreamCursor {
                incarnation: "inc".to_owned(),
                position: "1".to_owned(),
            },
            complete: true,
            replaces: false,
            snapshot_id: None,
            grants: vec![nessa_protocol::product::generated::GrantFrameEntry {
                conversation_id: marker.to_owned(),
                absent: false,
                capabilities: Some("read".to_owned()),
                policy_revision: Some("1".to_owned()),
            }],
        }
    }

    fn push(
        events: &mut Events,
        changed: &GrantsChanged,
    ) -> Result<(), crate::read_only_sync::application::GatewayError> {
        let event = EventFrame::push(product_event::GRANTS_CHANGED, changed, 1, 1).unwrap();
        events.observe(event)
    }

    #[test]
    fn one_grant_frame_past_the_count_closes_the_inbox() {
        let mut events = events();
        for index in 0..QUEUED_GRANT_FRAMES {
            push(&mut events, &frame("1", &index.to_string())).unwrap();
        }
        let extra = push(&mut events, &frame("1", "extra"));
        assert!(extra.is_err());
        assert_eq!(events.grants.len(), QUEUED_GRANT_FRAMES);
        let first = events.pop_grant().unwrap();
        assert_eq!(first.grants[0].conversation_id, "0");
        push(&mut events, &frame("1", "after")).unwrap();
    }

    #[test]
    fn one_grant_frame_past_the_byte_bound_is_not_kept() {
        let mut events = events();
        let mut heavy = frame("1", "heavy");
        heavy.grants[0].capabilities = Some("x".repeat(QUEUED_GRANT_BYTES));
        assert!(push(&mut events, &heavy).is_err());
        assert!(events.grants.is_empty());
        push(&mut events, &frame("1", "small")).unwrap();
        assert_eq!(events.grants.len(), 1);
    }

    #[test]
    fn a_grant_frame_for_no_subscription_is_not_queued() {
        let mut events = events();
        events.grant_opening = false;
        push(&mut events, &frame("1", "stray")).unwrap();
        assert!(events.grants.is_empty());
    }
}
