//! `nessa relay serve`: a relay both a gateway and its readers connect out
//! to, so a reader that cannot reach the gateway directly is joined to it
//! (issue #266, `docs/design/auth/relay.md`).
//!
//! ```text
//! composition::relay_command ──▶ infrastructure::serve_tcp (the listening socket)
//!                             ──▶ application::Relay (registrations, joins, limits)
//! application::Relay ──▶ crate::timer (every deadline and ping)
//!                    ──▶ RelayEntropy (challenges and join tokens)
//!                    ──▶ RelayProofs: nessa_auth's verify_relay_proof (who may register a key)
//! ```
//!
//! Arrows are construction and the calls each makes. The relay reads only
//! the preamble (`nessa_protocol::relay`); once two connections are joined
//! it copies their bytes and reads none of them. What crosses is TLS
//! between the reader and the gateway, which the relay cannot open, so it
//! holds no authority and stores nothing.
pub(crate) mod application;
pub(crate) mod infrastructure;
