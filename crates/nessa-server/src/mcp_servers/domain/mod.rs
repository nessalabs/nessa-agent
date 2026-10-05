//! Pure rules: a user's server as stored and the edits made to the stored
//! list, what stands in for a configured server, which stand-ins are let
//! through, the token that ties one to its conversation, which calls an MCP
//! App may make, and the ticket that redeems a resource it read.
mod app_call;
mod configured_server;
mod resource_ticket;
mod session_token;
mod stand_in;
pub use app_call::{
    admit_app, admit_tool_call, AppCallAdmission, AppFacts, AppRefusal, MAX_APP_ARGUMENTS_BYTES,
    MAX_APP_RESULT_BYTES,
};
pub use configured_server::{
    ConfiguredMcpServer, EditRefusal, EnvironmentNameRepeated, ServerEdit, ServerSave, StdioServer,
    MANAGED_SERVER_NAME,
};
pub use resource_ticket::{resource_ticket, ResourceTicketDigest};
pub use session_token::{session_token, TokenDigest};
pub use stand_in::{
    admit, configuration_digest, relay_arguments, stored_revision, ConfigurationKey,
    StandInRefusal, RELAY_SUBCOMMAND,
};
