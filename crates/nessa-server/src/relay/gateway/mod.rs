//! A gateway serving through a relay: registered under its own key, with
//! each reader the relay joins handed to the native listener on a relay
//! permit of its own (issue #266, slice R2a).
//!
//! ```text
//! composition::native_pairing ──▶ application::ThroughRelay::serve (registration, accept legs)
//!                             ──▶ infrastructure::RelayedAccept (the listener's second source)
//! ThroughRelay ──▶ RelayDial: TcpRelayDial
//!              ──▶ RelayKey: IdentityKey, the gateway's native identity
//!              ──▶ RelayAdmission: the listener's relay permits
//!              ──▶ crate::timer (every deadline and wait)
//! ```
//!
//! Arrows are construction and the calls each makes.
pub(crate) mod application;
pub(crate) mod infrastructure;
