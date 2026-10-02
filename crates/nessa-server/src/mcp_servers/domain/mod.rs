//! Pure rules: what stands in for a configured server, which stand-ins are
//! let through, the token that ties one to its conversation, which calls an
//! MCP App may make, and the ticket that redeems a resource it read.
mod app_call;
mod resource_ticket;
mod session_token;
mod stand_in;
pub use app_call::{
    admit_resource_read, admit_tool_call, AppCallAdmission, AppFacts, AppRefusal,
    MAX_APP_ARGUMENTS_BYTES, MAX_APP_RESULT_BYTES,
};
pub use resource_ticket::{resource_ticket, ResourceTicketDigest};
pub use session_token::{session_token, TokenDigest, SESSION_VARIABLE};
pub use stand_in::{
    admit, configuration_digest, relay_arguments, StandInRefusal, RELAY_SUBCOMMAND,
};
