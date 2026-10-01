use std::{error::Error, fmt};

/// Why a request to an MCP server, or a stand-in's connection to it, failed.
///
/// Branch on the variant; the text is for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpError {
    /// The servers given could not all be launched as configured
    /// ([`StdioMcpServer::all_valid`](crate::infrastructure::acp::sessions::StdioMcpServer::all_valid)).
    InvalidConfiguration,
    /// No server is configured under that name.
    NotConfigured,
    /// The server's process could not be launched; the text says why.
    Start(String),
    /// The server refused `initialize`, answered it with a protocol version
    /// this client does not speak, or answered something unreadable.
    Handshake(String),
    /// No answer within the request's deadline. The request was cancelled
    /// upstream; the connection stays.
    Timeout,
    /// The server's process ended, or its pipes closed, before an answer.
    ServerGone,
    /// The server answered with a JSON-RPC error.
    Remote {
        /// The JSON-RPC error code.
        code: i64,
        /// The server's message.
        message: String,
    },
    /// An answer, or a frame, was not of the shape the protocol defines.
    Malformed(String),
    /// Something was larger than its bound; the text names it.
    TooLarge(&'static str),
    /// The resource is not an MCP App: its MIME type is not
    /// `text/html;profile=mcp-app`.
    NotAnApp,
    /// Too many requests are already waiting on this server; nothing was sent.
    Busy,
    /// The MCP client is shutting down.
    Stopped,
}
impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                f.write_str("invalid MCP server name, executable or arguments")
            }
            Self::NotConfigured => f.write_str("no MCP server is configured under that name"),
            Self::Start(reason) => write!(f, "the MCP server could not be started: {reason}"),
            Self::Handshake(reason) => write!(f, "the MCP server's handshake failed: {reason}"),
            Self::Timeout => f.write_str("the MCP server did not answer in time"),
            Self::ServerGone => f.write_str("the MCP server ended"),
            Self::Remote { code, message } => {
                write!(f, "the MCP server answered with error {code}: {message}")
            }
            Self::Malformed(reason) => write!(f, "the MCP server's answer was malformed: {reason}"),
            Self::TooLarge(what) => write!(f, "{what} is larger than its bound"),
            Self::NotAnApp => {
                f.write_str("the resource is not an MCP App (text/html;profile=mcp-app)")
            }
            Self::Busy => f.write_str("too many requests are waiting on the MCP server"),
            Self::Stopped => f.write_str("the MCP client is shutting down"),
        }
    }
}
impl Error for McpError {}
