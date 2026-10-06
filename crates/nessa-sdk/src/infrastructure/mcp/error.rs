use crate::infrastructure::acp::sessions::McpServerProblem;
use std::{error::Error, fmt};

/// Why a request to an MCP server, or a stand-in's connection to it, failed.
///
/// Branch on the variant; the text is for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpError {
    /// The servers given could not all be launched as configured; the
    /// problem says why
    /// ([`McpServerLaunch::problem_in`](crate::infrastructure::mcp::McpServerLaunch::problem_in)).
    InvalidConfiguration(McpServerProblem),
    /// No server is configured under that name.
    NotConfigured,
    /// The server under that name is configured differently now than it was
    /// when the opening was admitted
    /// ([`McpServers::open_as`](crate::infrastructure::mcp::McpServers::open_as)).
    ConfigurationChanged,
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
    /// DNS, TCP, TLS, a 5xx answer, or the opening deadline passed before a
    /// session existed. Nothing was retained.
    Unreachable,
    /// The server refused the call as unauthorized (HTTP 401). No token is
    /// carried in the error.
    Unauthorized,
    /// The server answered 403 `insufficient_scope`. The call is not retried.
    InsufficientScope,
    /// The upstream session id is no longer recognized (HTTP 404 on a
    /// session-bound request). In-flight calls on that epoch fail with this;
    /// a later call may use a replacement session when the owner still admits
    /// one recovery.
    SessionExpired,
    /// The stream ended before a pending request's result was observed. That
    /// is not a cancellation receipt.
    Unconfirmed,
    /// Another opening already holds this upstream session id at the same MCP
    /// endpoint. This opening is refused and does not delete the other id.
    SessionCollision,
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
    /// The SDK session (a conversation's) has no open session of that
    /// server: its harness has not started one, or it ended.
    NoSession,
    /// The session was closed: its harness session ended, it was closed, or
    /// the grant it was opened under was revoked — which is also what an
    /// opening under a revoked grant is refused with.
    Closed,
}
impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(problem) => problem.fmt(f),
            Self::NotConfigured => f.write_str("no MCP server is configured under that name"),
            Self::ConfigurationChanged => {
                f.write_str("the MCP server is configured differently now")
            }
            Self::Start(reason) => write!(f, "the MCP server could not be started: {reason}"),
            Self::Handshake(reason) => write!(f, "the MCP server's handshake failed: {reason}"),
            Self::Timeout => f.write_str("the MCP server did not answer in time"),
            Self::ServerGone => f.write_str("the MCP server ended"),
            Self::Unreachable => f.write_str("the remote MCP server could not be reached"),
            Self::Unauthorized => f.write_str("the remote MCP server requires authorization"),
            Self::InsufficientScope => {
                f.write_str("the remote MCP server requires a broader scope")
            }
            Self::SessionExpired => f.write_str("the remote MCP session expired"),
            Self::Unconfirmed => {
                f.write_str("the remote MCP request ended without a confirmed result")
            }
            Self::SessionCollision => {
                f.write_str("the remote MCP server reused a session id another opening holds")
            }
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
            Self::NoSession => f.write_str("there is no open session of that MCP server here"),
            Self::Closed => f.write_str("the MCP session was closed"),
        }
    }
}
impl Error for McpError {}
