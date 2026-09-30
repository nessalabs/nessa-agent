//! Constructs trusted gateway dependencies once per server and the CLI client
//! adapter for online commands. Offline bootstrap is isolated in auth_command.
//! `read_only_example` composes the standalone Cargo example's private cache,
//! clock and output; its offline branches receive no credential or connector.
//!
//! ```text
//! Environment -> private runtime config -> auth + ConversationService
//!                                   -> receiver authority + owner repository
//!                                   -> fixed providers + OpenCode static profile
//!                                   -> current-agent resolver -> managed adapter + native store
//!                                   -> shared CLI/gateway native installer
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
mod managed_adapter;
#[cfg(unix)]
mod opencode_profile;

mod runtime_config;

mod agent;

// Where its consumers are: the provider's agent budgets and the conversation
// service's deletion budgets are both built only with Unix process supervision.
#[cfg(unix)]
mod agent_budgets;
#[cfg(unix)]
mod attachments;

mod desktop;

mod provisioning;

#[cfg(unix)]
mod warm_up;

mod cli;
mod read_only_example;
pub use read_only_example::run_read_only_example;
