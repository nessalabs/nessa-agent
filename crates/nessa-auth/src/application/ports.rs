//! Implement these ports in local or hosted adapters. Choose adapters in composition.
use crate::domain::{
    Action, AudienceId, AuthContext, Credential, CredentialId, Membership, Resource,
};
use std::{fmt, future::Future, pin::Pin};

/// Runtime-neutral boxed future borrowing its adapter and input for `'a`.
/// Adapters return typed failures and must not hide unavailable state as success.
pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AccessError>> + Send + 'a>>;

/// Private credential bytes: deliberately no Serialize, Clone, Display, or derived Debug.
/// This wrapper redacts diagnostics; it does not promise memory zeroization.
pub struct CredentialEvidence(Vec<u8>);
impl CredentialEvidence {
    /// Take ownership of `bytes`; reject empty evidence and values over 16 KiB.
    /// Construction checks size only, not authenticity.
    pub fn new(bytes: Vec<u8>) -> Result<Self, AccessError> {
        if bytes.is_empty() || bytes.len() > 16 * 1024 {
            return Err(AccessError::InvalidCredential);
        }
        Ok(Self(bytes))
    }
    /// Borrow secret bytes for verification. Never log or serialize this slice.
    pub fn expose_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl fmt::Debug for CredentialEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CredentialEvidence([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Application failures without provider-specific payloads or credential secrets.
pub enum AccessError {
    /// Proof is invalid, expired, revoked, or not yet valid.
    InvalidCredential,
    /// A previously authenticated credential has been revoked.
    CredentialRevoked,
    /// A previously authenticated session or credential has expired.
    CredentialExpired,
    /// The linked membership is disabled.
    InactiveMembership,
    /// Credential, audience, or membership identifiers do not agree.
    IdentityMismatch,
    /// A required verifier, store, or policy authority is unavailable.
    Unavailable,
    /// The selected adapter cannot perform this operation.
    Unsupported,
}
impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AccessError {}

/// A trusted adapter verifies the secret/issuer/audience and resolves its binding.
/// It cannot turn an arbitrary input DTO into a trusted application context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCredential {
    /// Verified Nessa binding ID; never a raw unverified provider subject.
    pub credential_id: CredentialId,
    /// Exclusive proof expiration in Unix seconds. At this instant the proof is invalid.
    pub expires_at: Option<u64>,
}

/// Adapter that verifies proof validity and resolves a Nessa credential binding.
/// Hosted adapters must validate issuer and audience before returning a binding.
/// No DTO mapping or credential-ID lookup alone satisfies this contract.
pub trait CredentialVerifier: Send + Sync {
    /// Verify `evidence` for the server-selected `audience`, not a client-selected
    /// trust policy. Return its binding and exclusive proof expiry, or a typed
    /// failure. This does not check current Nessa membership or authorize an action.
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential>;
}

/// Credential and membership must come from one committed authorization revision.
#[derive(Debug, Clone)]
pub struct AccessSnapshot {
    /// Current secret-free credential metadata.
    pub credential: Credential,
    /// Membership linked to the credential actor and organization.
    pub membership: Membership,
    /// Committed authorization-state version used for this snapshot.
    pub revision: u64,
}
/// Read current credential and membership metadata from one committed revision.
pub trait AccessReader: Send + Sync {
    /// Resolve `credential` and its corresponding membership coherently.
    /// Missing records or an unavailable authority must return an error. Session
    /// assembly rechecks linkage. Reads starting after a mutation publishes must
    /// observe that revision or newer; concurrent reads may observe either complete
    /// revision. Returned snapshots remain owned and unchanged after publication.
    fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot>;
}

/// Absolute time for credential expiration, distinct from the server uptime clock.
pub trait Clock: Send + Sync {
    /// Current absolute Unix time in seconds, used for expiry rather than uptime.
    fn unix_seconds(&self) -> u64;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Result of evaluating one action against current policy and credential limits.
pub enum Decision {
    /// Current policy permits this specific request.
    Allow,
    /// Current policy does not permit this request.
    Deny,
}

/// The application supplies authoritative resource ownership and current state.
/// A session context alone never implies current permission.
pub trait PolicyEvaluator: Send + Sync {
    /// Evaluate `action` on authoritative `resource` for the verified `context`.
    /// `snapshot` must be current and correspond to that context. Implementations
    /// must enforce tenant ownership, active membership, and credential limits;
    /// missing/invalid policy data must not produce Allow. This port has no default
    /// implementation. The caller owns admission ordering and expiry rechecks.
    fn evaluate(
        &self,
        context: &AuthContext,
        action: &Action,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Decision, AccessError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_is_bounded_and_redacted() {
        assert!(CredentialEvidence::new(vec![]).is_err());
        assert!(CredentialEvidence::new(vec![0; 16 * 1024 + 1]).is_err());
        assert_eq!(
            format!(
                "{:?}",
                CredentialEvidence::new(b"private".to_vec()).unwrap()
            ),
            "CredentialEvidence([REDACTED])"
        );
    }
}
