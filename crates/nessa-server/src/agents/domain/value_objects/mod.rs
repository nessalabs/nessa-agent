//! Immutable agent identities and the readiness they are reported in.
//! `Readiness` is constructed from the host's answers and owns the rule that
//! turns them into one thing to tell the person.
mod agent_id;
pub use agent_id::AgentId;
mod host_answer;
pub use host_answer::HostAnswer;
mod readiness;
pub use readiness::Readiness;
