//! The relay socket, the `mcp-relay` command on its other end, and the view's
//! tool UI lookup over the SDK's `McpServers`.
mod relay;
mod relay_command;
mod tool_uis;

#[cfg(test)]
pub(crate) use relay::said;

#[cfg(unix)]
pub use relay::bind;
pub use relay::{
    read_line, write_line, Answer, Hello, Refusal, Relay, ANSWER_TIMEOUT, HELLO_TIMEOUT,
    MAX_HELLO_BYTES,
};
#[cfg(unix)]
pub use relay_command::run;
pub use relay_command::{relay, RelayFailure};
pub use tool_uis::ListedToolUis;
