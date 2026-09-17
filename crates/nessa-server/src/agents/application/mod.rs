//! Reading an agent's readiness through a port the infrastructure fills in.
//! Use case -> `AgentProbe` (host questions); answers -> domain `Readiness`.
//! The port answers with a typed failure so "no" and "could not tell" stay apart.
mod ports;
pub use ports::{AgentProbe, ProbeFailure};
mod readiness;
pub use readiness::ReadAgentReadiness;
