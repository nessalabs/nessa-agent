//! Ports, discovery and the per-server owner.
//!
//! ```text
//! AuthorizationOwner ──step──▶ domain::ServerAuth
//!                   ──▶ AuthorizationRecords
//!                   ──▶ AuthorizationAudit
//!                   ──▶ OAuthHttp
//!                   ──▶ ConsentCallback
//! ```
mod discovery;
mod owner;
mod ports;

pub use discovery::{challenge_scope, discover, https_url, resource_metadata, s256};
pub use owner::AuthorizationOwner;
pub use ports::{
    AuditFailure, AuthAuditRecord, AuthClock, AuthorizationAudit, AuthorizationHandoff,
    AuthorizationRecords, AuthorizeAnswer, BindingChange, CallbackBind, CallbackQuery,
    ConsentCallback, Entropy, FenceRefusal, ListedAuthorization, OAuthCallFailure, OAuthHttp,
    OAuthResponse, PermissiveHandoff, RecordFailure, ResourceLookup, RevokeAnswer, SessionDrain,
    TokenMaterial,
};
