//! Bounded local WebSocket client using the same product protocol as other surfaces.
//! `mcp` is the stdio server; each tool call is [`crate::cli::application::invoke`].
mod gateway;
mod mcp;
pub use gateway::LocalGateway;
pub use mcp::serve as serve_mcp;
