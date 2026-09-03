//! Configuration load failures from [`super::environment`].
//!
//! `ReadError` — problem reading a single env var from an [`super::source::EnvSource`].
//! `EnvironmentError` — full config parse failure (invalid stage, port, missing token, etc.).
//!
//! Re-exported at `crate::env::EnvironmentError`.

use super::stage::{InvalidStage, Stage};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    NotUnicode { key: String },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotUnicode { key } => write!(f, "{key} is not valid unicode"),
        }
    }
}

impl std::error::Error for ReadError {}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    Read {
        variable: &'static str,
        source: ReadError,
    },
    InvalidPort {
        variable: &'static str,
        value: String,
    },
    InvalidStage(InvalidStage),
    Empty {
        variable: &'static str,
    },
    Required {
        variable: &'static str,
        stage: Stage,
    },
    InsecureBind {
        bind_host: String,
        stage: Stage,
    },
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { variable, source } => write!(f, "{variable}: {source}"),
            Self::InvalidPort { variable, value } => {
                write!(f, "{variable} must be a valid TCP port, got {value:?}")
            }
            Self::InvalidStage(error) => write!(f, "{error}"),
            Self::Empty { variable } => write!(f, "{variable} must not be empty"),
            Self::Required { variable, stage } => {
                write!(f, "{variable} is required for the {} stage", stage.as_str())
            }
            Self::InsecureBind { bind_host, stage } => write!(
                f,
                "binding to {bind_host:?} is not allowed in the {} stage until secure transport (wss/TLS) is configured; use loopback",
                stage.as_str()
            ),
        }
    }
}

impl std::error::Error for EnvironmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidStage(error) => Some(error),
            _ => None,
        }
    }
}
