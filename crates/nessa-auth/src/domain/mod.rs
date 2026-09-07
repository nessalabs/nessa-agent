//! Identity, membership, and credential rules, independent of storage and transport.
//!
//! Start with the identifiers, then build models from validated values. Constructors
//! enforce structural invariants; they do not verify a secret or grant authority.
//! Only application session assembly constructs an [`AuthContext`].
//!
//! ```text
//! Principal ── Membership ── Organization
//!     │                         │
//! Credential ── Grant ── Resource
//!     │
//! Audience (the intended deployment)
//! ```
//!
//! - `identifiers`: opaque, bounded IDs and action names. No provider ID format.
//! - `models`: ownership, membership, lifetime, and revocation invariants.
//! - `auth_context`: stable identity selectors after application verification.
//! - `error`: validation failures without raw credential secrets.
//!
//! This module imports neither serde nor application DTOs. Time values are Unix
//! seconds supplied by callers; domain objects never read a clock themselves.
mod auth_context;
mod error;
mod identifiers;
mod models;

pub use auth_context::AuthContext;
pub use error::DomainError;
pub use identifiers::{
    Action, AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, ResourceId,
};
pub use models::{
    Credential, Grant, Membership, MembershipRole, MembershipStatus, Organization, Principal,
    PrincipalKind, Resource,
};
