use super::ApprovalAttribution;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    permissions::{PermissionId, PermissionOptionId},
};

/// Host-authorized selection of one offered option for an exact pending review.
/// This command is not an execution retry or proof of tool delivery. The Agent
/// checks correlation through its provider; the adapter audits selection and write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAnswer {
    /// Host-verified actor and decision basis. Never deserialize untrusted claims directly.
    pub attribution: ApprovalAttribution,
    /// Execution that owns the review; stale executions must not resolve later reviews.
    pub execution_id: ExecutionId,
    /// Pending permission identity within that execution.
    pub id: PermissionId,
    /// Exact offered option identity; arbitrary or previously resolved choices fail.
    pub option_id: PermissionOptionId,
}
