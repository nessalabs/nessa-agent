//! What an agent is, and what stands between it and running. No transport,
//! no filesystem: the host's answers arrive as values and leave as a readiness.
pub mod value_objects;
pub use value_objects::{AgentId, HostAnswer, Readiness};
