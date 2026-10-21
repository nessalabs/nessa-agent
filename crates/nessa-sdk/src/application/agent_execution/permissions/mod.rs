//! Permission controls retain host attribution and exact reviewed tool input.
//! Audit records retain cancellation causes and attributed answer delivery.
//! Room refusal evidence consumes domain open-question accounting derived from
//! the adapter's retained asks; this layer does not own its limits or fit policy.
//!
//! ```text
//! host --> permission command --> adapter --> ExecutionController
//!                                   |
//!                                   +--> ExecutionAudit
//! refusal evidence --> ExecutionController::decline_review --> ReviewDeclineRecord
//!                  --> DeclinedReview (same rule) --> the event the panel reads
//!                  --> with_delivery (same identity, refusal and stopping rule)
//! configured refusal --> admit_configured_outcome --> OperationCapabilities
//! permission deadline --> ApprovalBasis::Deadline (actor permission-deadline)
//! remembered rule --> PermissionRules::settle --> ApprovalBasis::Rule
//! waiting review --> ReviewNotices (cleared on answer, cancel, deadline)
//! ```
//!
//! Arrows show control flow. The adapter asks the controller to resolve a review
//! and submits decision and delivery evidence to the audit port. The resolution
//! retains the controller’s session; delivery records derive ownership from it. Attribution is supplied by
//! the verified host; these types do not authenticate actors or implement policy.
//! A remembered rule is the host's stored decision, asked through `PermissionRules`;
//! this module does not evaluate it.

#![deny(missing_docs)]

mod answer;
mod approval;
mod audit;
mod cancellation;
mod outcome;
mod rules;
pub use answer::{
    PermissionAnswer, PermissionAnswerFailure, PermissionAnswerFuture, PermissionAnswerResult,
    PermissionSelectionState, QuestionAnswer,
};
pub(crate) use approval::cap_hook_reason;
pub use approval::{
    ActionContext, ApprovalAttribution, ApprovalBasis, ApprovalModeSnapshot, ApprovalRuleReference,
    HookRefusal, PermissionResolution,
};
pub use audit::{
    DeclinedReview, PermissionAnswerDelivery, PermissionAnswerRecord, QuestionAnswerRecord,
    QuestionRefusalRecord, RefusedAsk, ReviewDeclineRecord,
};
pub(crate) use cancellation::validate_hook_withdrawal;
pub use cancellation::{CancellationOrigin, PermissionCancellation, PermissionCancellationRequest};
pub use outcome::{admit_configured_outcome, ConfiguredRefusal, StopKind};
pub use rules::{
    is_remember_option, remember_effect, IdlePermissionRules, IdleReviewNotices, PermissionCaller,
    PermissionRules, RememberError, RememberRequest, ReviewNotice, ReviewNotices, SettledRule,
    REMEMBER_ALLOW, REMEMBER_DENY,
};
