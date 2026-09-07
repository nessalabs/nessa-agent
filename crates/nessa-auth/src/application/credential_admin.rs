//! Provider-neutral credential lifecycle commands.

use super::{
    dto::{CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, PrincipalInputDto},
    ports::{AccessError, CredentialEvidence, PortFuture},
};

#[derive(Clone, Debug, PartialEq, Eq)]
/// Request to issue one exact credential, with retry identity separate from its secret.
pub struct IssueCredentialRequest {
    /// Caller-generated idempotency key scoped to `issuer_principal_id`.
    pub request_id: String,
    /// Verified issuing principal used to scope the idempotency receipt.
    pub issuer_principal_id: String,
    /// Server-selected stable credential identifier.
    pub credential_id: String,
    /// Existing or newly proposed non-admin principal.
    pub principal: PrincipalInputDto,
    /// Active member relationship for the same principal and organization.
    pub membership: MembershipInputDto,
    /// Server-selected deployment audience.
    pub audience_id: String,
    /// Inclusive issuance time in Unix seconds.
    pub issued_at: u64,
    /// Caller-requested exclusive expiry in Unix seconds.
    pub expires_at: Option<u64>,
    /// Exact action and resource restrictions requested by the caller.
    pub grants: Vec<CredentialGrantDto>,
}

/// The evidence variant is returned exactly once and cannot be serialized.
pub enum IssueCredentialOutcome {
    /// Newly committed credential and its sole secret delivery.
    Issued {
        /// Public, secret-free credential metadata.
        metadata: CredentialMetadataDto,
        /// One-time bearer evidence; this value is never persisted as plaintext.
        evidence: CredentialEvidence,
    },
    /// A matching command was already committed and its secret cannot be replayed.
    ExistingSecretUnavailable {
        /// Public metadata for explicit revoke-and-reissue recovery.
        metadata: CredentialMetadataDto,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Query public credentials owned by one verified organization.
pub struct ListCredentialsRequest {
    /// Organization whose metadata may be listed after caller authorization.
    pub organization_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Idempotently revoke one exact credential after caller authorization.
pub struct RevokeCredentialRequest {
    /// Caller-generated idempotency key scoped to `issuer_principal_id`.
    pub request_id: String,
    /// Verified principal requesting revocation.
    pub issuer_principal_id: String,
    /// Exact credential to revoke.
    pub credential_id: String,
    /// Revocation time in Unix seconds.
    pub revoked_at: u64,
}

/// Lifecycle failures distinguish rejected commands from unavailable storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialAdminError {
    /// Request identity or payload conflicts with existing state; do not retry unchanged.
    Conflict,
    /// A configured resource limit prevents this command.
    Capacity,
    /// The requested credential does not exist.
    NotFound,
    /// The authority could not read or durably commit its state.
    Unavailable,
}
impl std::fmt::Display for CredentialAdminError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for CredentialAdminError {}

/// Managed-credential operations remain separate from credential verification.
pub trait CredentialAdmin: Send + Sync {
    /// Commit issuance before returning a one-time secret.
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError>;
    /// Return secret-free metadata for the requested organization.
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError>;
    /// Commit an idempotent revocation and return the registry revision.
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, u64, CredentialAdminError>;
}

/// Read the current committed authorization revision without exposing storage.
pub trait AuthRevisionSource: Send + Sync {
    /// Return the latest committed registry revision.
    fn revision(&self) -> Result<u64, AccessError>;
    /// Subscribe to committed revisions. Receivers may coalesce by reading the
    /// latest revision after any notification.
    fn subscribe(&self) -> Result<std::sync::mpsc::Receiver<u64>, AccessError>;
}
