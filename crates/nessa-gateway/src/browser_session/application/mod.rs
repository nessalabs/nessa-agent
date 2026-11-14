//! Session storage port, current-origin admission, and sign-in orchestration.
//!
//! Entrypoints -> application use cases/ports -> domain session state
//!
//! The arrows show transport code delegating coordination while authoritative
//! origin and lifetime invariants remain in the domain.
mod session;
pub use session::{
    invalidation_reason, BrowserSessionVerifier, ReadBrowserSession, SessionStore, SignIn,
};
