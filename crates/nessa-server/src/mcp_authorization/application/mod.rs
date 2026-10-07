//! Ports, discovery and the per-server owner.
//!
//! ```text
//! AuthorizationOwner ──step──▶ domain::ServerAuth
//!                   ──▶ AuthorizationRecords
//!                   ──▶ AuthorizationAudit
//!                   ──▶ OAuthHttp
//!                   ──▶ ConsentCallback
//! ```
//!
//! ConsentCallback delivers bounded candidates. The owner applies the domain
//! callback decision once and drops the receiver on acceptance or a terminal
//! outcome before completing persistence, audit and exchange effects.
mod discovery;
mod owner;
mod ports;

pub use discovery::{challenge_scope, discover, https_url, resource_metadata, s256};
pub use owner::AuthorizationOwner;
pub use ports::{
    AdmissionRefusal, AdmittedToken, AuditFailure, AuthAuditRecord, AuthClock, AuthorizationAudit,
    AuthorizationHandoff, AuthorizationRecords, AuthorizeAnswer, BindingChange, CallbackBind,
    CallbackQuery, ConsentCallback, Entropy, FenceRefusal, ListedAuthorization, OAuthCallFailure,
    OAuthHttp, OAuthResponse, PermissiveHandoff, RecordFailure, ResourceLookup, RevokeAnswer,
    SessionDrain, TokenMaterial,
};
