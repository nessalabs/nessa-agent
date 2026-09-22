//! Publishes the gateway's actual bound loopback endpoint and verifies it again
//! at local client process boundaries.
//!
//! The file is discovery evidence. A reader returns its address only when the
//! same process and optional managed-runtime identity answer on unauthenticated
//! health. That correlation rejects stale and mismatched listeners; it is not
//! cryptographic authentication, which remains the session handshake's job.
//!
//! ```text
//! bound listener ──publish──▶ private JSON file
//!                                  │
//! local client ◀──correlate── file + GET /health
//! ```
//! Arrows show publication and verified discovery across process boundaries.

pub mod application;
pub mod domain;
pub mod infrastructure;
