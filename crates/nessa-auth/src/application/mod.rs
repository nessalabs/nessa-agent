//! Connect external evidence to domain identity through injected dependencies.
//!
//! ```text
//! Input DTO ── mapping ── validated domain value (still not authority)
//!
//! Evidence ── CredentialVerifier ── credential binding + proof expiry
//!                                      │
//!                                 AccessReader
//!                                      │
//!                            credential + membership + revision
//!                                      │
//!                         AuthenticateSession + Clock
//!                                      │
//!                              AuthenticatedSession
//!
//! Opaque session ID ── SessionVerifier ── credential binding
//!                                            │
//!                                    ResumeSession
//!                                            │
//!                         current AuthenticatedSession + snapshot
//! ```
//!
//! `dto` defines serialized input shapes; `mapping` validates their domain values.
//! `ports` defines the contracts implemented by local or hosted adapters. `session`
//! verifies credential evidence or opaque session proof before returning a context.
//! A credential identifier alone is never proof. Composition owns adapter selection
//! and lifetime; this module opens no socket or database.
//!
//! Authentication is not ongoing authorization. A gateway must apply current
//! policy to each action and handle expiry and revision invalidation. A DTO that
//! says “admin” is never permission to update a membership.

pub mod credential_admin;
pub mod credential_registry;
pub mod dto;
mod mapping;
pub mod ports;
pub mod session;

/// Recheck current access and evaluate policy for one action.
pub mod authorization;
