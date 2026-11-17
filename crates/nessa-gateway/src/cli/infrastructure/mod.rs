//! Bounded local WebSocket client using the same product protocol as other surfaces.
//! `mcp` is the stdio server; each tool call is [`crate::cli::application::invoke`].
mod gateway;
mod mcp;
mod wait;
pub use gateway::LocalGateway;
pub use mcp::serve as serve_mcp;
pub use wait::surface_call_budget;
