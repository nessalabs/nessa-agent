//! Models review choices, their allowed scopes, and once-only resolution of a
//! request for one execution and tool. Host attribution remains in application.
//!
//! ```text
//! PermissionOfferPolicy --> PermissionOptions --> PermissionRequest --> PermissionStateView
//!                                    |
//!                                    +--> ReviewDeclineObservation
//! PermissionDeadlinePolicy --> class deadline for an unanswered request
//! ```
//!
//! Arrows mean filtering offered choices, constructing a request, and resolving
//! it. The branch is the request that never became one: a tool this binding
//! will not put to a host, a frame it could not describe, or no choice left to
//! offer. That is a decision too, so its immutable selection and response-write
//! stages are modelled under one checked identity rather than lost.
mod authority;
pub(crate) use authority::{OpenQuestions, PendingPermissions, QuestionListing};
pub use authority::{
    PendingQuestion, PendingReview, PermissionAuthority, PermissionAuthorityError,
};
pub mod entities;
pub mod value_objects;
pub use entities::{PermissionRequest, PermissionStateView};
pub use value_objects::{
    wait_ms, CustomPermissionCancellationReason, DeadlineDenyPreference, DeadlineReason,
    PermissionApplicationId, PermissionCancellationReason, PermissionCancellationReasonView,
    PermissionDeadlinePolicy, PermissionDeadlineSettlement, PermissionDecision, PermissionEffect,
    PermissionId, PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionReviewDeadline, PermissionScope, PermissionScopeView, PermissionSessionId,
    PermissionToolClass, RefusalOutcome, ReviewContent, ReviewDecline, ReviewDeclineId,
    ReviewDeclineObservation, ReviewDeclineReason, ReviewDeclineStage, MAX_DEADLINE_DUE_MS,
    MAX_DEADLINE_REASON_BYTES, MAX_OPEN_REVIEWS, MAX_PERMISSION_DEADLINE, MAX_REVIEW_OPTIONS,
    MAX_REVIEW_OPTION_LABEL_BYTES, MAX_REVIEW_TOOL_NAME_BYTES, STANDARD_ALLOW_REASON,
    STANDARD_DENY_REASON, STANDARD_STOP_REASON,
};
