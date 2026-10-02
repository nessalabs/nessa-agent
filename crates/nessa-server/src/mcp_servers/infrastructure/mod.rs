//! The relay socket, the `mcp-relay` command on its other end, the grants
//! that tie each stand-in to its conversation, and the view's tool UI lookup
//! over the SDK's `McpServers`.
mod grants;
mod relay;
mod relay_command;
mod tool_uis;

#[cfg(all(test, unix))]
pub(crate) use relay::said;

pub use grants::ConversationGrants;
#[cfg(unix)]
pub use relay::{bind, BoundRelay};
pub use relay::{
    read_line, write_line, Answer, Hello, Refusal, Relay, ANSWER_TIMEOUT, HELLO_TIMEOUT,
    MAX_HELLO_BYTES,
};
#[cfg(unix)]
pub use relay_command::run;
pub use relay_command::{relay, RelayFailure};
pub use tool_uis::ListedToolUis;
