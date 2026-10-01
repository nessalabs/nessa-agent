//! `nessa mcp-relay SOCKET SERVER CONFIGURATION`: the stand-in a harness
//! runs in place of a configured MCP server. It says hello on the relay
//! socket and then copies bytes: its stdin to the gateway, the gateway's
//! answers to its stdout. Its stdout is the MCP stream and nothing else;
//! diagnostics go to stderr.
use super::relay::{read_line, write_line, Answer, Hello, Refusal, ANSWER_TIMEOUT};
use std::fmt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Why a stand-in ended without serving.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayFailure {
    /// The relay socket could not be reached: no gateway is serving it.
    Unreachable,
    /// The gateway did not answer the hello in time, or not in one line.
    NoAnswer,
    /// The gateway turned the stand-in away.
    Refused { reason: Refusal, message: String },
}
impl fmt::Display for RelayFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("the Nessa gateway's MCP relay is not reachable"),
            Self::NoAnswer => f.write_str("the Nessa gateway did not answer the MCP stand-in"),
            Self::Refused { reason, message } => {
                write!(
                    f,
                    "the Nessa gateway refused the MCP stand-in ({reason:?}): {message}"
                )
            }
        }
    }
}

/// Run the stand-in for `server` over the socket at `socket`, on this
/// process's stdin and stdout, until either side ends.
#[cfg(unix)]
pub async fn run(
    socket: &std::path::Path,
    server: &str,
    configuration: &str,
) -> Result<(), RelayFailure> {
    let connection = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|_| RelayFailure::Unreachable)?;
    relay(
        connection,
        server,
        configuration,
        tokio::io::stdin(),
        tokio::io::stdout(),
    )
    .await
}

/// The stand-in over any `connection`, copying `input` to it and its answers
/// to `output`. Ends when the gateway closes; the harness closing its input
/// is passed on, and then the gateway closes.
pub async fn relay(
    connection: impl AsyncRead + AsyncWrite + Unpin,
    server: &str,
    configuration: &str,
    mut input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> Result<(), RelayFailure> {
    let (from_gateway, mut to_gateway) = tokio::io::split(connection);
    let mut from_gateway = BufReader::new(from_gateway);
    let hello = Hello {
        server: server.into(),
        configuration: configuration.into(),
    };
    write_line(&mut to_gateway, &hello)
        .await
        .map_err(|_| RelayFailure::Unreachable)?;
    let answer = tokio::time::timeout(ANSWER_TIMEOUT, read_line::<Answer>(&mut from_gateway))
        .await
        .ok()
        .flatten()
        .ok_or(RelayFailure::NoAnswer)?;
    if let Answer::Refused { reason, message } = answer {
        return Err(RelayFailure::Refused { reason, message });
    }
    let upstream = async {
        let _ = tokio::io::copy(&mut input, &mut to_gateway).await;
        let _ = to_gateway.shutdown().await;
        // The harness is done; the gateway's close ends the stand-in.
        std::future::pending::<()>().await;
    };
    let downstream = async {
        let _ = tokio::io::copy(&mut from_gateway, &mut output).await;
        let _ = output.flush().await;
    };
    tokio::select! {
        () = upstream => {}
        () = downstream => {}
    }
    Ok(())
}
