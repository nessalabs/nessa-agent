//! Reading an agent's question off ACP, and writing back what it was answered.
//!
//! ```text
//! elicitation/create --> wire::question --> AgentQuestion
//! answer             --> wire::accepted | declined | cancelled
//! ```
//!
//! Arrows mean translation at the boundary: the schema an agent sends becomes a
//! domain question, and a host's answer becomes the content that schema asked
//! for. Nothing here decides what to answer.
pub(crate) mod wire;
