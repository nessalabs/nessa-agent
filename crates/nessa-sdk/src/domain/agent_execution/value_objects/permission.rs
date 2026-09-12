//! Decisions apply to one request, never a standing grant of authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    RejectOnce,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Pending,
    Answered(PermissionDecision),
    Cancelled,
}
