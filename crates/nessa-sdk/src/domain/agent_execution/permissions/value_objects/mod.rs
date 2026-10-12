//! Validated permission identities, choices, immutable scopes, and cancellation causes.
//! Configuration describes permitted choices without approving an action.
//! A deadline says how long an unanswered review waits and which outcome it takes.
//!
//! ```text
//! PermissionOfferPolicy --> PermissionOptions --> decision or cancellation cause
//! PermissionDeadlinePolicy --> PermissionReviewDeadline --> settlement option or cancel
//! declined review -------> ReviewDeclineObservation(selected -> final write stage)
//! ```
//!
//! Arrows mean filtering review choices, classifying an unanswered review, and
//! describing their eventual resolution.
mod deadline;
mod identity;
mod permission;
mod review_decline;
pub(crate) use deadline::checked_deadline_reason;
pub use deadline::{
    wait_ms, DeadlineDenyPreference, DeadlineReason, PermissionDeadlinePolicy,
    PermissionDeadlineSettlement, PermissionReviewDeadline, PermissionToolClass,
    MAX_DEADLINE_REASON_BYTES, MAX_PERMISSION_DEADLINE, STANDARD_ALLOW_REASON,
    STANDARD_DENY_REASON, STANDARD_STOP_REASON,
};
pub use identity::{
    PermissionApplicationId, PermissionId, PermissionOptionId, PermissionSessionId,
};
pub use permission::{
    CustomPermissionCancellationReason, PermissionCancellationReason,
    PermissionCancellationReasonView, PermissionDecision, PermissionEffect, PermissionOfferPolicy,
    PermissionOption, PermissionOptions, PermissionScope, PermissionScopeView,
};
pub use review_decline::{
    RefusalOutcome, ReviewDecline, ReviewDeclineId, ReviewDeclineObservation, ReviewDeclineReason,
    ReviewDeclineStage,
};
