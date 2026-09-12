//! Agent execution is an injected application port. These types describe Nessa
//! requests and controls; domain types own prompts and agent-turn observations.
//! No provider protocol enters this layer.
//!
//! caller --> Agent --> capability validation --> AgentSession
//!                                             <-- observations
//!
//! The arrows show calls and results. The host supplies a binding and policy;
//! this module does not authenticate callers or own durable conversation state.
mod contracts;
mod service;
pub use contracts::*;
pub(crate) use service::validate_prompt;
pub use service::Agent;

mod mapping;
