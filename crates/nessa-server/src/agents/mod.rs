//! Which coding agents can actually run here, and what is stopping the rest.
//!
//! Setup offers a choice of agent, and an offer the runtime cannot honour is
//! worse than no offer: it fails later, somewhere the person cannot connect
//! back to the choice they made. So the choice is made against what is
//! installed and signed in on this machine.
//!
//! The server answers rather than the desktop shell, because the server is the
//! process that would actually launch the agent — what it can see is what will
//! be true when the agent runs. It answers *before* authentication, because
//! this is the question asked while setting Nessa up, when there is no session
//! yet and nothing to authenticate with.
//!
//! Nothing here reads a secret. It asks whether a credential exists, never
//! what it is.

pub mod adapters;
pub mod application;
pub mod domain;
pub mod entrypoint;
