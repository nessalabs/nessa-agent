//! Constructs trusted gateway dependencies once per server and the CLI client
//! adapter for online commands. Offline bootstrap is isolated in auth_command.
//!
//! ```text
//! Environment -> private runtime config -> auth + ConversationService
//!                                   -> one provider per configured agent
//!                                   -> storage / audit
//!                                   -> attachments (one store, shared)
//!                                   -> AgentWarmUp -> readiness port
//! ProductRouteState -> authenticated HTTP/WebSocket router
//! ```
//! Arrows show construction and injection. Conversations share the service across
//! sockets; shutdown closes its Agents before the process exits.

mod root;

pub use root::CompositionRoot;

mod auth_command;
mod credential_registry;
mod install_command;
mod installed_launch;
mod local_auth;

mod runtime_config;

mod agent;

// Everywhere, because the conversation service's deletion budgets come from
// it on every platform; the agent budgets inside it are Unix-only, where their
// one consumer, the provider, is.
mod agent_budgets;
mod attachments;

mod desktop;

mod provisioning;

mod warm_up;

mod cli;
