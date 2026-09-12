use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    EmptyValue(&'static str),
    InvalidPath,
    DifferentTool,
    DifferentExecution,
    PermissionResolved,
    NoPermissionOptions,
    DuplicatePermissionOption,
    DuplicatePermissionDecision,
    UnknownPermissionOption,
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent execution: {self:?}")
    }
}
impl Error for ExecutionError {}
