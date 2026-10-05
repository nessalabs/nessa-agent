//! The blocking socket a native pairing connection runs on, at either end.
//!
//! ```text
//! deadline_stream --> wake (stop flag per socket, WAKE_TICK)
//!                 --> Clock (monotonic; injected)
//! worker (JoinError -> PairingWorkerFault)
//! ```
//! Arrows are compile-time dependencies. This is the one part of
//! `nessa-protocol` that touches a socket: blocking std socket mechanics both
//! ends need, which a copy at each end would let drift.
mod deadline_stream;
mod wake;
mod worker;
pub use deadline_stream::{
    begin_enrollment_phase, check_deadline, BufferedIo, DeadlineError, DeadlineStream,
    NativeDeadline, NativeSocket, ENROLLMENT_DEADLINE, TLS_DEADLINE,
};
pub use wake::{
    NativeWakeCause, NativeWakeOutcome, NativeWakeReport, WakeEndpoint, WakeEndpoints,
    NATIVE_CONNECTION_CAPACITY, WAKE_TICK,
};
pub use worker::worker_fault;
