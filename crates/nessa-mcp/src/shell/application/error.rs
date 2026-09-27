//! Command admission failure before a runner is allowed to start.
use std::{error::Error, fmt};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellError {
    AdmissionAuditFailed,
}
impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdmissionAuditFailed => {
                f.write_str("admission audit failed; command was not started")
            }
        }
    }
}
impl Error for ShellError {}
