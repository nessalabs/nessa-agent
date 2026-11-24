//! Allocation ordering and ports. The accepted task owns receipts through caller loss.
mod cloud_boxes;
mod runtime_settings;
mod workspaces;
pub use cloud_boxes::{
    ActiveWorkChecker, BoxFuture, CloudBoxAudit, CloudBoxAuditEvent, CloudBoxBackend,
    CloudBoxProvider, CloudBoxStore, CloudBoxes, ProvisionedCloudBox,
};
pub use runtime_settings::{
    RuntimeAudit, RuntimeAuditEvent, RuntimeCaller, RuntimeChange, RuntimePublication,
    RuntimeSettings,
};
pub use workspaces::{
    Agent37Enrollment, Assignments, ChoiceCause, CloudChoice, Provider, RecoveryAudit,
    RecoveryEvent, RestCause, ServedCopy, Settings, WorkspaceReceipt, WorkspaceRuntime,
    WorkspaceStream, Workspaces,
};
