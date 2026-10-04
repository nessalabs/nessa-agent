use crate::product_contract::generated::{
    CatalogueReadErrorCode, RecordReadErrorCode, SessionCloseReason,
};
use std::io::{Read, Result as IoResult, Write};
use std::net::SocketAddr;
use std::time::Duration;

/// Cancellation supplied by the composition that owns the synchronous run.
pub(crate) trait Cancellation: Send + Sync {
    fn cancelled(&self) -> bool;
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
