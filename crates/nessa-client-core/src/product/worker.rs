//! The sole physical pump and its canonical retained-session handshake adapter.
use super::ports::{ProductConnector, ProductStream};
use super::session::{lock, Owner, ProductCompletion, ProductFailure};
use crate::protected::Channel;
use crate::read_only_sync::application::{
    Cancellation, GatewayConnector, GatewayError, GatewayPolicy, GatewayStream,
};
use crate::read_only_sync::infrastructure::gateway::{DeviceEvidence, Session};
use nessa_auth::adapters::pairing::NativeIdentity;
use nessa_protocol::product::generated::{product_event, ProductSurfaceKind};
use nessa_protocol::product::handshake::accepted_termination;
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use std::io::{Error, Read, Result as IoResult, Write};
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct Open {
    pub(super) address: SocketAddr,
    pub(super) identity: Arc<NativeIdentity>,
    pub(super) pin: [u8; 44],
    pub(super) credential: String,
    pub(super) instance: String,
    pub(super) connector: Arc<dyn ProductConnector>,
}
struct Stop(Arc<Owner>);
impl Cancellation for Stop {
    fn cancelled(&self) -> bool {
        lock(&self.0.state).closing
    }
    fn record_failure(&self, error: GatewayError) {
        self.0.record(error.into());
    }
}
struct Connector<'a> {
    open: &'a Open,
    owner: Arc<Owner>,
}
impl GatewayConnector for Connector<'_> {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        let connected = self.open.connector.connect(address, timeout)?;
        let stopped = {
            let mut state = lock(&self.owner.state);
            state.interrupt = Some(connected.interrupt.clone());
            state.closing
        };
        // Install the independent handle before Session starts pinned TLS. A
        // close that arrived during OS connect stops this same original socket.
        // The interrupt is injected; the owner records a panic from it.
        if stopped {
            self.owner.interrupt(connected.interrupt.as_ref());
        }
        Ok(Box::new(Stream {
            inner: connected.stream,
            owner: self.owner.clone(),
        }))
    }
}
struct Stream {
    inner: Box<dyn ProductStream>,
    owner: Arc<Owner>,
}
fn guarded<T>(owner: &Owner, operation: impl FnOnce() -> IoResult<T>) -> IoResult<T> {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|_| {
        owner.record(ProductFailure::WorkerPanicked);
        Err(Error::other("native product stream callback panicked"))
    })
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        guarded(&self.owner, || self.inner.read(bytes))
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        guarded(&self.owner, || self.inner.write(bytes))
    }
    fn flush(&mut self) -> IoResult<()> {
        guarded(&self.owner, || self.inner.flush())
    }
}
impl GatewayStream for Stream {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        guarded(&self.owner, || self.inner.read_timeout(timeout))
    }
    fn write_timeout(&self, timeout: Duration) -> IoResult<()> {
        guarded(&self.owner, || self.inner.write_timeout(timeout))
    }
    fn shutdown(&self) -> IoResult<()> {
        guarded(&self.owner, || self.inner.shutdown())
    }
}
pub(super) fn run(owner: Arc<Owner>, open: Open) {
    let cause = catch_unwind(AssertUnwindSafe(|| pump(&owner, &open)))
        .unwrap_or(Err(ProductFailure::WorkerPanicked))
        .err()
        .unwrap_or(ProductFailure::Cancelled);
    // pump's socket has dropped before the owner announces physical drain.
    owner.finish(cause);
}
fn pump(owner: &Arc<Owner>, open: &Open) -> Result<(), ProductFailure> {
    let limits = owner.limits;
    let connector = Connector {
        open,
        owner: owner.clone(),
    };
    let session = Session::connect_surface(
        open.address,
        DeviceEvidence {
            identity: &open.identity,
            pin: open.pin,
            credential: &open.credential,
        },
        "app.nessa.mobile",
        ProductSurfaceKind::Panel,
        &open.instance,
        &connector,
        owner.clock.clone(),
        Arc::new(Stop(owner.clone())),
        GatewayPolicy::new(limits.handshake_ms, limits.request_ms, limits.events)?,
    )?;
    let (mut channel, ready) = session.into_product_channel();
    // Retain the factual opening even when a simultaneous stop fences new work.
    lock(&owner.state).ready = Some(Arc::new(ready));
    owner.notify();
    // The borrowed channel survives result and panic capture. Record the first
    // known refusal before Drop starts possibly delayed physical shutdown.
    let result = catch_unwind(AssertUnwindSafe(|| exchange(owner, &mut channel)))
        .unwrap_or(Err(ProductFailure::WorkerPanicked));
    if let Err(cause) = result {
        owner.record(cause);
    }
    result
}
fn exchange(owner: &Arc<Owner>, channel: &mut Channel) -> Result<(), ProductFailure> {
    let limits = owner.limits;
    loop {
        {
            let mut state = lock(&owner.state);
            if state.closing {
                return Err(state.failure.unwrap_or(ProductFailure::Cancelled));
            }
            if state.active.is_none() {
                state.active = state.queue.pop_front().map(|ticket| (ticket, false));
            }
        }
        let active = lock(&owner.state).active.clone();
        if let Some((ticket, false)) = &active {
            if owner.clock.elapsed_ms() >= ticket.deadline {
                lock(&owner.state).active.take();
                ticket.complete(ProductCompletion::NotDispatched(ProductFailure::TimedOut));
                continue;
            }
            {
                let mut state = lock(&owner.state);
                if state.closing {
                    return Err(state.failure.unwrap_or(ProductFailure::Cancelled));
                }
                state.active.as_mut().expect("pump owns active").1 = true;
            }
            // Record dispatch before the first write: write failure is uncertain.
            channel
                .transport
                .stream_mut()
                .begin_operation(ticket.deadline);
            channel.send(&ticket.text)?;
        }
        let deadline = active.as_ref().map(|(ticket, _)| ticket.deadline);
        channel
            .transport
            .stream_mut()
            .begin_poll(deadline, limits.poll_ms, limits.frame_ms);
        let Some(text) = channel.poll()? else {
            continue;
        };
        let message = OutgoingMessage::decode(&text).map_err(|_| ProductFailure::Protocol)?;
        match message {
            OutgoingMessage::Response(response) => {
                let Some((ticket, _)) = active else {
                    return Err(ProductFailure::Correlation);
                };
                if response.id != ticket.id.to_string() {
                    return Err(ProductFailure::Correlation);
                }
                lock(&owner.state).active.take();
                ticket.complete(ProductCompletion::Response(Box::new(response)));
            }
            OutgoingMessage::Event(event) => {
                if let Some(termination) = session_termination(&event) {
                    return Err(termination);
                }
                let mut state = lock(&owner.state);
                let bytes = state
                    .event_bytes
                    .checked_add(text.len())
                    .ok_or(ProductFailure::Capacity)?;
                if state.event_count >= limits.events || bytes > limits.event_bytes {
                    return Err(ProductFailure::Capacity);
                }
                state.event_bytes = bytes;
                state.event_count += 1;
                state.events.push_back((event, text.len()));
                drop(state);
                owner.notify();
            }
        }
    }
}

/// `session.terminated` is the connection's close, read before any caller
/// event is queued. A malformed payload is a protocol failure. Any other
/// event is caller data.
fn session_termination(event: &EventFrame) -> Option<ProductFailure> {
    if event.event != product_event::SESSION_TERMINATED {
        return None;
    }
    Some(match accepted_termination(&event.payload) {
        Some(termination) => ProductFailure::Closed(Some(termination.code)),
        None => ProductFailure::Protocol,
    })
}

#[cfg(test)]
mod tests {
    use super::session_termination;
    use nessa_protocol::product::generated::{product_event, SessionTermination};
    use nessa_protocol::product_contract::generated::SessionCloseReason;
    use nessa_protocol::protocol::EventFrame;
    use serde_json::json;

    fn frame(payload: &serde_json::Value) -> EventFrame {
        EventFrame::push(product_event::SESSION_TERMINATED, payload, 1, 0).unwrap()
    }

    #[test]
    fn every_termination_reason_closes_and_a_malformed_one_does_not_queue() {
        for reason in [
            SessionCloseReason::AuthenticationFailed,
            SessionCloseReason::CredentialRevoked,
            SessionCloseReason::CredentialExpired,
            SessionCloseReason::AuthorizationLost,
            SessionCloseReason::ProtocolIncompatible,
            SessionCloseReason::HandshakeTimeout,
            SessionCloseReason::TemporaryUnavailable,
            SessionCloseReason::GatewayRestarting,
            SessionCloseReason::GatewayOverloaded,
            SessionCloseReason::ServerShutdown,
            SessionCloseReason::TransportInterrupted,
        ] {
            let event = frame(
                &serde_json::to_value(SessionTermination {
                    code: reason,
                    retryable: reason.retryable(),
                    retry_after_ms: None,
                })
                .unwrap(),
            );
            let failure = session_termination(&event);
            assert_eq!(failure, Some(super::ProductFailure::Closed(Some(reason))));
            assert_eq!(
                failure.unwrap().connection_outcome(),
                Some(nessa_protocol::product_contract::connection_outcome(Some(
                    reason
                )))
            );
        }
        let ordinary = EventFrame::push("conversation.listed", &json!({}), 1, 0).unwrap();
        assert_eq!(session_termination(&ordinary), None);
        assert_eq!(
            session_termination(&frame(
                &json!({"code":"credential_revoked","retryable":true})
            )),
            Some(super::ProductFailure::Protocol)
        );
        assert_eq!(
            session_termination(&frame(&json!({
                "code": "temporary_unavailable",
                "retryable": true,
                "retryAfterMs": 60_001
            }))),
            Some(super::ProductFailure::Protocol)
        );
        assert_eq!(
            super::ProductFailure::Closed(None).connection_outcome(),
            Some(nessa_protocol::product_contract::ConnectionOutcome::Retryable)
        );
        assert_eq!(super::ProductFailure::Protocol.connection_outcome(), None);
    }
}
