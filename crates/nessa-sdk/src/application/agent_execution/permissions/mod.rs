//! Permission controls retain host attribution and exact reviewed tool input.
//! Audit records retain cancellation causes and attributed answer delivery.
//!
//! ```text
//! host --> permission command --> adapter --> ExecutionController
//!                                   |
//!                                   +--> ExecutionAudit
//! ```
//!
//! Arrows show control flow. The adapter asks the controller to resolve a review
//! and submits decision and delivery evidence to the audit port. The resolution
//! retains the controller’s session; delivery records derive ownership from it. Attribution is supplied by
//! the verified host; these types do not authenticate actors or implement policy.

#![deny(missing_docs)]

mod answer;
mod approval;
mod audit;
mod cancellation;
pub use answer::{
    PermissionAnswer, PermissionAnswerFailure, PermissionAnswerFuture, PermissionAnswerResult,
    PermissionSelectionState, QuestionAnswer,
};
pub use approval::{
    ActionContext, ApprovalAttribution, ApprovalBasis, ApprovalModeSnapshot, ApprovalRuleReference,
    PermissionResolution,
};
pub use audit::{
    PermissionAnswerDelivery, PermissionAnswerRecord, QuestionAnswerRecord, ReviewDeclineRecord,
};
pub use cancellation::{CancellationOrigin, PermissionCancellation, PermissionCancellationRequest};
