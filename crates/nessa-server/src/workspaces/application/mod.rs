//! Allocation ordering and ports. The accepted task owns receipts through caller loss.
mod workspaces;
pub(crate) use workspaces::{
    Assignments, Provider, RecoveryAudit, RecoveryEvent, Settings, WorkspaceReceipt,
    WorkspaceRuntime, WorkspaceStream, Workspaces,
};
