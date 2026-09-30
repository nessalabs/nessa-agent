use crate::product_contract::generated::{
    CatalogueReadErrorCode, RecordReadErrorCode, SessionCloseReason,
};

use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    time::Duration,
};

/// Cancellation supplied by the composition that owns the synchronous run.
pub(crate) trait Cancellation: Send + Sync {
    fn cancelled(&self) -> bool;
}

/// The synchronous connection owner supplies socket I/O and timeout controls.
pub(crate) trait GatewayStream: Read + Write {
    fn read_timeout(&self, timeout: Duration) -> io::Result<()>;
    fn write_timeout(&self, timeout: Duration) -> io::Result<()>;
    fn shutdown(&self) -> io::Result<()>;
}
pub(crate) trait GatewayConnector {
    fn connect(&self, address: SocketAddr, timeout: Duration)
        -> io::Result<Box<dyn GatewayStream>>;
}

/// Finite physical policy; protocol/semantic ceilings remain their publications.
#[derive(Clone, Copy)]
pub(crate) struct GatewayPolicy {
    handshake_ms: u64,
    operation_ms: u64,
    upgrade_bytes: usize,
    unexpected_events: usize,
    control_frames: usize,
}
impl GatewayPolicy {
    pub(crate) fn new(
        handshake_ms: u64,
        operation_ms: u64,
        upgrade_bytes: usize,
        unexpected_events: usize,
        control_frames: usize,
    ) -> Result<Self, GatewayError> {
        if handshake_ms == 0 || operation_ms == 0 || upgrade_bytes == 0 {
            return Err(GatewayError::InvalidPolicy);
        }
        Ok(Self {
            handshake_ms,
            operation_ms,
            upgrade_bytes,
            unexpected_events,
            control_frames,
        })
    }
    pub(crate) fn handshake_ms(self) -> u64 {
        self.handshake_ms
    }
    pub(crate) fn operation_ms(self) -> u64 {
        self.operation_ms
    }
    pub(crate) fn upgrade_bytes(self) -> usize {
        self.upgrade_bytes
    }
    pub(crate) fn unexpected_events(self) -> usize {
        self.unexpected_events
    }
    pub(crate) fn control_frames(self) -> usize {
        self.control_frames
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
    UpgradeTooLarge,
    ResponseTooLarge,
    RequestTooLarge,
    EventCapacity,
    ControlCapacity,
    InvalidCredential,
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
