//! Models review choices, their allowed scopes, and once-only resolution of a
//! request for one execution and tool. Host attribution remains in application.
//!
//! ```text
//! PermissionOfferPolicy --> PermissionOptions --> PermissionRequest --> PermissionStateView
//!                                    |
//!                                    +--> ReviewDeclineObservation
//! ```
//!
//! Arrows mean filtering offered choices, constructing a request, and resolving
//! it. The branch is the request that never became one: a tool this binding
//! will not put to a host, a frame it could not describe, or no choice left to
//! offer. That is a decision too, so its immutable selection and response-write
//! stages are modelled under one checked identity rather than lost.
pub mod entities;
pub mod value_objects;
pub use entities::{PermissionRequest, PermissionStateView};
pub use value_objects::{
    CustomPermissionCancellationReason, PermissionApplicationId, PermissionCancellationReason,
    PermissionCancellationReasonView, PermissionDecision, PermissionEffect, PermissionId,
    PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope, PermissionScopeView, PermissionSessionId, ReviewDecline, ReviewDeclineId,
    ReviewDeclineObservation, ReviewDeclineReason, ReviewDeclineStage,
};
