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
//!                         AuthenticateSession + Clock + PolicyEvaluator
//!                                      │
//!                              AuthenticatedSession
//!
//! Opaque session ID ── SessionVerifier ── credential binding
//!                                            │
//!                                    ResumeSession
//!                                            │
//!                         Cedar session entry + current AuthenticatedSession + snapshot
//! ```
//!
//! `dto` defines serialized input shapes; `mapping` validates their domain values.
//! `ports` defines the contracts implemented by local or hosted adapters. `session`
//! verifies credential evidence or opaque session proof before returning a context.
//! A credential identifier alone is never proof. Composition owns adapter selection
//! and lifetime; this module opens no socket or database.
//!
//! `origin` validates the original retained issuance and its command identity;
//! readers expose it with current metadata under the existing access fence.
//! The session-entry policy consumes its cause without granting operations.
//! `revocation` binds the actual original revocation to that same current read;
//! `credential_journal` owns shared sequence/revision/correlation decoding.
//! Neither value reconstructs an actor or creates another history owner.
//!
//! Authentication is not ongoing authorization. A gateway must apply current
//! policy to each action and handle expiry and revision invalidation. A DTO that
//! says “admin” is never permission to update a membership.

pub mod credential_admin;
mod credential_journal;
pub mod credential_registry;
pub mod dto;
mod mapping;
pub mod origin;
/// Bounded device enrollment through current authentication and policy.
pub mod pairing;
pub mod ports;
pub mod revocation;
pub mod session;

/// Recheck current access and evaluate policy for one action.
pub mod authorization;
