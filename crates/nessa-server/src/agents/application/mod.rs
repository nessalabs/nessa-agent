//! Reading an agent's readiness through a port the infrastructure fills in.
//! Use case -> `AgentProbe` (host questions); answers -> domain `Readiness`.
//! The port answers with a typed failure so "no" and "could not tell" stay apart.
//!
//! `SharedAgentReadiness` is what an entrypoint asks. It bounds what the
//! question costs: concurrent callers all want the same parameterless answer, so
//! one probe runs and they share it, and nobody waits for it past a deadline.
mod ports;
pub use ports::{
    AgentCredential, AgentCredentialFailure, AgentCredentialKind, AgentCredentialSource,
    AgentProbe, ProbeFailure,
};
mod readiness;
pub use readiness::ReadAgentReadiness;
mod shared_readiness;
pub use shared_readiness::{ReadingFailure, SharedAgentReadiness, READINESS_DEADLINE};
