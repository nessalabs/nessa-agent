//! Validated permission identities, choices, immutable scopes, and cancellation causes.
//! Configuration describes permitted choices without approving an action.
//!
//! ```text
//! PermissionOfferPolicy --> PermissionOptions --> decision or cancellation cause
//! ```
//!
//! Arrows mean filtering review choices and describing their eventual resolution.
mod identity;
mod permission;
mod review_decline;
pub use identity::{
    PermissionApplicationId, PermissionId, PermissionOptionId, PermissionSessionId,
};
pub use permission::{
    CustomPermissionCancellationReason, PermissionCancellationReason,
    PermissionCancellationReasonView, PermissionDecision, PermissionEffect, PermissionOfferPolicy,
    PermissionOption, PermissionOptions, PermissionScope, PermissionScopeView,
};
pub use review_decline::{ReviewDecline, ReviewDeclineReason};
