//! Models review choices, their allowed scopes, and once-only resolution of a
//! request for one execution and tool. Host attribution remains in application.
//!
//! ```text
//! PermissionOfferPolicy --> PermissionOptions --> PermissionRequest --> PermissionStateView
//! ```
//!
//! Arrows mean filtering offered choices, constructing a request, and resolving it.
pub mod entities;
pub mod value_objects;
pub use entities::{PermissionRequest, PermissionStateView};
pub use value_objects::{
    CustomPermissionCancellationReason, PermissionApplicationId, PermissionCancellationReason,
    PermissionCancellationReasonView, PermissionDecision, PermissionEffect, PermissionId,
    PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope, PermissionScopeView, PermissionSessionId,
};
