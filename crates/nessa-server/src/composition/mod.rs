//! Constructs trusted gateway dependencies once per server and the CLI client
//! adapter for online commands. Offline bootstrap is isolated in auth_command.
//!
//! ```text
//! Environment -> private runtime config -> auth + ConversationService
//!                                         -> provider / storage / audit
//! ProductRouteState -> authenticated HTTP/WebSocket router
//! ```
//! Arrows show construction and injection. Conversations share the service across
//! sockets; shutdown closes its Agents before the process exits.

mod root;

pub use root::CompositionRoot;

mod auth_command;
mod local_auth;

mod runtime_config;

mod agent;

mod desktop;

mod cli;
