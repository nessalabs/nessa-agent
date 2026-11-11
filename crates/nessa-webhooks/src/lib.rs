//! Outbound webhook delivery of committed conversation records (issue 896):
//! registrations, the private durable outbox, signed message bytes and the
//! bounded HTTPS sender.
//!
//! `application -> domain` owns delivery transitions; `infrastructure ->
//! application` signs and posts exact bytes. Authorization is supplied by the
//! conversation and auth owners in nessa-server, never inferred from an
//! endpoint or a MAC key.
// The one clock rule nessa-server holds (clippy.toml), kept here since this
// code moved out of it.
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]
pub mod application;
pub mod domain;
pub mod infrastructure;
