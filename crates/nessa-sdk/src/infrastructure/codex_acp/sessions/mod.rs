//! Configures and opens a Codex context through shared ACP sessions.
//!
//! ```text
//! host config -> CodexAcpProvider -> profile -> ACP session
//!                                     |
//!                          Codex selections and their checks
//! ```
//! Arrows show construction and validation. Process startup, resume, and the
//! launch-input fingerprint that identifies a restorable context remain shared
//! ACP responsibilities, not provider-specific lifecycle implementations.
//! By default `CodexAcpProvider` puts `features.hooks: false` and `notify: []`
//! in the launch configuration. `with_native_hooks` omits both keys on a
//! process start that reads the switch on. The profile reports native hook
//! suppression as unsupported
//! either way, because that request is not the effective setting and omitting
//! it is not proof either. Startup and restoration still complete.
//! `deletion` says what a successful `session/delete` means for Codex: the
//! adapter archives the thread, which is not erasing it.
mod binding;
mod deletion;
mod profile;
pub use binding::{CodexAcpProvider, PROVIDER_NAME};
