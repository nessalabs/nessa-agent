//! Fatal process errors — config, bind, and serve failures.
//!
//! Returned from [`super::bootstrap::run`] and [`crate::composition::CompositionRoot::serve`].
//! Implements [`std::process::Termination`] so `main` can exit with a logged message.
//!
//! Re-exported at `crate::core::RunError`.

use crate::env::EnvironmentError;
use std::fmt;
use std::io::{self, ErrorKind};

/// Fatal errors that stop the server process.
#[derive(Debug)]
pub enum RunError {
    Environment(EnvironmentError),
    Bind { addr: String, source: io::Error },
    Serve(io::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(error) => write!(f, "invalid configuration: {error}"),
            Self::Bind { addr, source } => match source.kind() {
                ErrorKind::AddrInUse => write!(
                    f,
                    "port already in use at {addr}; stop the other process or set NESSA_PORT"
                ),
                _ => write!(f, "failed to bind {addr}: {source}"),
            },
            Self::Serve(source) => write!(f, "server stopped: {source}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Environment(error) => Some(error),
            Self::Bind { source, .. } => Some(source),
            Self::Serve(source) => Some(source),
        }
    }
}

impl From<EnvironmentError> for RunError {
    fn from(value: EnvironmentError) -> Self {
        Self::Environment(value)
    }
}

impl From<io::Error> for RunError {
    fn from(value: io::Error) -> Self {
        Self::Serve(value)
    }
}

impl std::process::Termination for RunError {
    fn report(self) -> std::process::ExitCode {
        tracing::error!(%self, "nessa-server failed");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{EnvironmentError, TOKEN};

    #[test]
    fn display_environment_error() {
        let error = RunError::Environment(EnvironmentError::Empty { variable: TOKEN });
        assert!(error.to_string().contains(TOKEN));
    }
}
