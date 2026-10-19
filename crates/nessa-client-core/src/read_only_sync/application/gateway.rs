use nessa_protocol::product_contract::generated::{
    CatalogueReadErrorCode, ConversationErrorCode, RecordReadErrorCode, SessionCloseReason,
};
use nessa_protocol::product_contract::{connection_outcome, ConnectionOutcome};
use std::io::{Read, Result as IoResult, Write};
use std::net::SocketAddr;
use std::time::Duration;

/// Cancellation supplied by the composition that owns the synchronous run.
pub(crate) trait Cancellation: Send + Sync {
    fn cancelled(&self) -> bool;
    /// The monotonic time, on the connection's clock, at which the run is
    /// cancelled whatever its operation's own deadline: a blocked read or
    /// write waits no longer than this. `None` when only `cancelled` ends it.
    fn until_ms(&self) -> Option<u64> {
        None
    }
    /// Publish the socket's first typed physical cause before cleanup can block.
    /// Retained runs keep it locally; product owners also fence racing close.
    fn record_failure(&self, _error: GatewayError) {}
}

/// The synchronous connection owner supplies socket I/O and timeout controls.
pub(crate) trait GatewayStream: Read + Write {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()>;
    fn write_timeout(&self, timeout: Duration) -> IoResult<()>;
    fn shutdown(&self) -> IoResult<()>;
}
pub(crate) trait GatewayConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>>;
}

/// Finite physical policy; protocol/semantic ceilings remain their publications.
#[derive(Clone, Copy)]
pub(crate) struct GatewayPolicy {
    handshake_ms: u64,
    operation_ms: u64,
    unexpected_events: usize,
}
impl GatewayPolicy {
    pub(crate) fn new(
        handshake_ms: u64,
        operation_ms: u64,
        unexpected_events: usize,
    ) -> Result<Self, GatewayError> {
        if handshake_ms == 0 || operation_ms == 0 {
            return Err(GatewayError::InvalidPolicy);
        }
        Ok(Self {
            handshake_ms,
            operation_ms,
            unexpected_events,
        })
    }
    pub(crate) fn handshake_ms(self) -> u64 {
        self.handshake_ms
    }
    pub(crate) fn operation_ms(self) -> u64 {
        self.operation_ms
    }
    pub(crate) fn unexpected_events(self) -> usize {
        self.unexpected_events
    }
}

/// Sanitized cause, retained by one attempt even when a core port maps it coarsely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayError {
    InvalidPolicy,
    InvalidRequest,
    DriverPanicked,
    Cancelled,
    TimedOut,
    Transport,
    Closed(Option<SessionCloseReason>),
    Protocol,
    Correlation,
    /// TLS refused the connection: the gateway key is not the pinned one, or
    /// the handshake failed. Not a statement about the device's credential.
    NativeHandshake,
    ResponseTooLarge,
    RequestTooLarge,
    EventCapacity,
    InvalidCredential,
    /// The gateway answered `openProduct` with the enrollment `Refused` reply:
    /// no product session for this connection (design rows PR2, PR10, PR13,
    /// PR15). The reply is redacted, so a key without an active credential is
    /// not told apart from a full pool here; the pinned status asked next
    /// tells them apart (row PC5).
    ProductRefused,
    Authentication(SessionCloseReason),
    ScopeChanged,
    Busy,
    Record(RecordReadErrorCode),
    Catalogue(CatalogueReadErrorCode),
    /// The gateway refused one `artifact.read` range with its typed code.
    HeldFile(HeldFileRefusal),
    /// The gateway refused `conversation.subscribe` or `conversation.unsubscribe`
    /// with a code this client was taught.
    Subscription(SubscriptionRefusal),
}

impl GatewayError {
    /// The shared lifecycle projection for either handshake refusal or close.
    /// Other gateway failures carry no published connection reason.
    pub(crate) fn connection_outcome(self) -> Option<ConnectionOutcome> {
        match self {
            Self::Closed(reason) => Some(connection_outcome(reason)),
            Self::Authentication(reason) => Some(connection_outcome(Some(reason))),
            _ => None,
        }
    }
}

/// How the gateway refuses a conversation subscription. A code this client
/// was not taught is a protocol failure, never stored here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubscriptionRefusal {
    /// The socket's access refusal, which is no conversation code.
    Forbidden,
    /// The socket's access refusal, which is no conversation code.
    Unauthorized,
    /// `subscription_capacity`, `subscription_duplicate`, `unknown_subscription`,
    /// or `cursor_ahead`.
    Subscription(SubscriptionCode),
    /// A conversation code, including `conversation_not_found` for a
    /// conversation this device was not granted.
    Conversation(ConversationErrorCode),
}

/// The subscription codes the product schema publishes, apart from the
/// conversation codes a subscribe is also refused with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubscriptionCode {
    Capacity,
    Duplicate,
    Unknown,
    CursorAhead,
}
impl SubscriptionCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Capacity => "subscription_capacity",
            Self::Duplicate => "subscription_duplicate",
            Self::Unknown => "unknown_subscription",
            Self::CursorAhead => "cursor_ahead",
        }
    }
}

/// How the gateway refuses an `artifact.read` range: a conversation code, or
/// the socket's own access refusals, which are no conversation code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeldFileRefusal {
    /// The gateway does not know `artifact.read`: it is older than held-file
    /// reads. Its `unknown_method` is never a `Conversation` code here.
    UnknownMethod,
    Conversation(ConversationErrorCode),
    Forbidden,
    Unauthorized,
}

/// One driver attempt's consumed evidence; no durable authentication decision.
pub(crate) struct GatewayOutcome {
    pub(crate) operation: u64,
    pub(crate) failure: Option<GatewayError>,
}

/// A returned callback value is separate from the retained transport evidence.
pub(crate) struct GatewayAttempt<R> {
    pub(crate) result: Option<R>,
    pub(crate) outcome: GatewayOutcome,
}
