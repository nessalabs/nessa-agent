//! Outbound delivery of committed conversation records (issue 896).
//!
//! The outbox, registrations, signing and HTTPS sending are the
//! `nessa_webhooks` crate. This module keeps the parts that meet the
//! conversation context: admission against current credential and share
//! facts, the canonical record source, and the runtime that drives them.
pub mod application;
pub mod infrastructure;
