//! The relay leg of a retained read's connect, and its race with the direct
//! dial (`docs/design/auth/relay.md`, "Direct, then relay").
//!
//! ```text
//! direct dial ──────────────▶ connected first: used, the relay never dialed
//!   not connected after DIRECT_HEAD_START, or refused sooner:
//!   relay leg: dial the relay ── Connect{K} ──▶ Joined: used
//!                                           ──▶ Offline or NoAnswer: refused
//! the first leg to connect is used; the other is dropped when it ends
//! ```
//!
//! Both legs share the connect's one budget. What a joined stream carries is
//! the same pinned TLS as a direct one, so the relay leg decides nothing
//! about the gateway: it only finds a path to it.
use nessa_protocol::relay::{
    read_frame, write_frame, GatewayKey, RelayAddress, RelayFrame, RelayRefusal,
};
use std::io::{Error, ErrorKind, Result as IoResult};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::STOP_SLICE;

/// Where a reader asks for its gateway when the direct dial does not
/// connect: the relay's `host:port`, and the pinned gateway key it asks for.
#[derive(Clone)]
pub(super) struct RelayRoute {
    pub(super) address: RelayAddress,
    pub(super) key: GatewayKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leg {
    Direct,
    Relay,
}

/// Run `direct` at once and `relay` once `direct` has not connected within
/// `head_start` or has failed, each given what is left of `timeout`, and
/// return the first to connect. Both failing returns the relay's error, the
/// later and more telling one. `stopped` is asked at least every
/// [`STOP_SLICE`]. A leg still running when the race returns is left to end
/// on its own, and what it connected is dropped: a dial ends at its
/// deadline, but the relay's host name lookup is bounded only by the
/// system resolver. A `timeout` no longer than `head_start` starts the relay
/// only after a refused direct dial.
pub(crate) fn race<T: Send + 'static>(
    direct: impl FnOnce(Duration) -> IoResult<T> + Send + 'static,
    relay: impl FnOnce(Duration) -> IoResult<T> + Send + 'static,
    timeout: Duration,
    head_start: Duration,
    stopped: &dyn Fn() -> bool,
) -> IoResult<T> {
    let started = Instant::now();
    let deadline = started
        .checked_add(timeout)
        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    let (sender, results) = mpsc::channel();
    spawn(Leg::Direct, direct, timeout, sender.clone())?;
    let mut relay = Some(relay);
    let mut relay_at = started + head_start.min(timeout);
    let mut running = 1;
    let mut direct_error = None;
    let mut relay_error = None;
    loop {
        if stopped() {
            return Err(Error::from(ErrorKind::ConnectionAborted));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(failed(relay_error, direct_error));
        }
        if now >= relay_at {
            if let Some(leg) = relay.take() {
                spawn(Leg::Relay, leg, deadline - now, sender.clone())?;
                running += 1;
            }
        }
        let mut wake = deadline.min(now + STOP_SLICE);
        if relay.is_some() {
            wake = wake.min(relay_at);
        }
        match results.recv_timeout(wake.saturating_duration_since(now)) {
            Ok((_, Ok(connected))) => return Ok(connected),
            Ok((leg, Err(error))) => {
                running -= 1;
                match leg {
                    // A refused direct dial need not wait out the head start.
                    Leg::Direct => {
                        relay_at = Instant::now();
                        direct_error = Some(error);
                    }
                    // A slow direct dial may still connect.
                    Leg::Relay => relay_error = Some(error),
                }
                if running == 0 && relay.is_none() {
                    return Err(failed(relay_error, direct_error));
                }
            }
            // Our own sender is held, so the channel is never disconnected.
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
    }
}

/// The relay's error where it ran, else the direct dial's, else a timeout.
fn failed(relay: Option<Error>, direct: Option<Error>) -> Error {
    relay
        .or(direct)
        .unwrap_or_else(|| Error::from(ErrorKind::TimedOut))
}

type LegResult<T> = (Leg, IoResult<T>);

fn spawn<T: Send + 'static>(
    leg: Leg,
    dial: impl FnOnce(Duration) -> IoResult<T> + Send + 'static,
    timeout: Duration,
    results: Sender<LegResult<T>>,
) -> IoResult<()> {
    thread::Builder::new()
        .name("nessa-reader-dial".into())
        .spawn(move || {
            // A race already decided drops what this leg connected.
            let _ = results.send((leg, dial(timeout)));
        })
        .map(drop)
}

/// Look up the relay at `route`, then dial it and ask it for the gateway's
/// key within `timeout`. The lookup itself waits as long as the system
/// resolver does; the race above bounds the caller. `Joined` returns the
/// stream with the gateway's own bytes next on it.
pub(super) fn relay_leg(route: &RelayRoute, timeout: Duration) -> IoResult<TcpStream> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    let left = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))
    };
    let mut stream = connect_any(route.address.as_str(), deadline)?;
    // Small preamble frames go out at once.
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(left()?))?;
    write_frame(&mut stream, &RelayFrame::Connect { key: route.key })?;
    stream.set_read_timeout(Some(left()?))?;
    let answer = read_frame(&mut stream)?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    match answer {
        RelayFrame::Joined => Ok(stream),
        other => Err(RelayRefusal::of(other)
            .map_or_else(|| Error::from(ErrorKind::InvalidData), Error::from)),
    }
}

/// Look up `address` (`host:port`) and connect to the first address it
/// resolves to that answers before `deadline`, trying each in turn, as a
/// TCP connect by name does. The lookup itself waits as long as the system
/// resolver does.
pub(super) fn connect_any(address: &str, deadline: Instant) -> IoResult<TcpStream> {
    let mut refused = None;
    for address in address.to_socket_addrs()? {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))?;
        match TcpStream::connect_timeout(&address, left) {
            Ok(connected) => return Ok(connected),
            Err(error) => refused = Some(error),
        }
    }
    Err(refused.unwrap_or_else(|| Error::from(ErrorKind::NotFound)))
}

#[cfg(test)]
#[path = "../../tests/retained/relay.rs"]
mod tests;
