//! Constructs trusted gateway dependencies once per server and the CLI client
//! adapter for online commands. Offline bootstrap is isolated in auth_command.
//!
//! ```text
//! Environment -> private runtime config -> auth + ConversationService
//!                                   -> fixed providers + OpenCode static profile
//!                                   -> current-agent resolver
//!                                   -> storage / audit
//!                                   -> attachments (one store, shared)
//!                                   -> fixed AgentWarmUp + current OpenCode lane
//!                                                      -> readiness port
//! ProductRouteState -> authenticated HTTP/WebSocket router
//! ```
//! Arrows show construction and injection. Conversations share the service across
//! sockets; shutdown closes its Agents before the process exits. Provider,
//! attachment, installed-launch, and warm-up composition exists only on Unix,
//! where the conversation process stack can run. Configuration parsing and
//! desktop defaults remain portable; non-Unix composition refuses conversations.

mod root;

pub use root::CompositionRoot;

mod auth_command;
mod credential_registry;
#[cfg(unix)]
mod current_agent;
mod install_command;
#[cfg(unix)]
mod installed_launch;
mod local_auth;
#[cfg(unix)]
mod opencode_profile;

mod runtime_config;

mod agent;

// Everywhere, because the conversation service's deletion budgets come from
// it on every platform; the agent budgets inside it are Unix-only, where their
// one consumer, the provider, is.
mod agent_budgets;
#[cfg(unix)]
mod attachments;

mod desktop;

mod provisioning;

#[cfg(unix)]
mod warm_up;

mod cli;
