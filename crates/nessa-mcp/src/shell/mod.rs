//! Shell execution tool. The application owns commands and audit ports; Shepherd is an adapter.
//! MCP transport -> application -> domain and injected runner/audit implementations.
pub mod application;
pub mod domain;
pub mod infrastructure;
