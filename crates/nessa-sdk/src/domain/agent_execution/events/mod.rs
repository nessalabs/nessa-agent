//! Semantic execution observations shared across providers and consumers.
//! Execution IDs correlate observations; these are not durable storage records.
//! Transport failures travel through the application port's error result.
mod agent_turn_event;
pub use agent_turn_event::{AgentTurnEvent, AgentTurnUpdate};
