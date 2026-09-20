//! Durable local credential registry and bearer-token verifier.
//!
//! The registry stores only SHA-256 verifiers for 256-bit random secrets. SHA-256
//! is appropriate here because secrets are uniformly random, not user passwords;
//! verification uses a constant-time comparison. The registry lock is held for
//! this store's lifetime and every mutation is persisted before publication.
//!
//! Every lifecycle change is committed together with its evidence: the registry
//! file carries an append-only `transitions` list produced by the domain and
//! checked by the same validator on write and on reopen. A failed write is a
//! failed commit, so there is no "committed but unaudited" state to reconcile.

use crate::{
    application::{
        credential_admin::{
            AuthRevisionSource, CredentialAdmin, CredentialAdminError, CredentialTransitionReader,
            IssueCredentialOutcome, IssueCredentialRequest, ListCredentialsRequest,
            ListTransitionsRequest, RevokeCredentialOutcome, RevokeCredentialRequest,
        },
        dto::{
            CredentialGrantDto, CredentialMetadataDto, CredentialTransitionDto, InitiatorDto,
            MembershipInputDto, MembershipRoleDto, MembershipStateDto, OrganizationInputDto,
            PrincipalInputDto, PrincipalKindDto, ResourceDto, TransitionCauseDto,
        },
        ports::{
            AccessError, AccessReader, AccessSnapshot, CredentialEvidence, CredentialVerifier,
            PortFuture, VerifiedCredential,
        },
    },
    domain::{
        AudienceId, Credential, CredentialId, CredentialTransition, DomainError, Initiator,
        IssuanceCause, Membership, PrincipalId, Supersession, TransitionCause,
    },
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, MutexGuard, RwLock,
    },
};
use subtle::ConstantTimeEq;

/// Schema 2 added the `transitions` list. A schema 1 file is upgraded in memory
/// on open and written back as schema 2 by the next mutation.
const SCHEMA_VERSION: u32 = 2;
const LEGACY_SCHEMA_VERSION: u32 = 1;
const TOKEN_PREFIX: &str = "nessa_v1";
/// Per-store resource bounds, injected by composition.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct LocalStoreConfig {
    pub max_registry_bytes: u64,
    pub max_credentials: usize,
    pub max_receipts: usize,
}
impl Default for LocalStoreConfig {
    fn default() -> Self {
        Self {
            max_registry_bytes: 4 * 1024 * 1024,
            max_credentials: 1_000,
            max_receipts: 2_000,
        }
    }
}
impl LocalStoreConfig {
    pub fn validate(&self) -> Result<(), LocalStoreError> {
        if self.max_registry_bytes == 0
            || self.max_registry_bytes >= usize::MAX as u64
            || self.max_credentials == 0
            || self.max_receipts == 0
        {
            return Err(LocalStoreError::Capacity);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum LocalStoreError {
    Locked,
    NotInitialized,
    AlreadyInitialized,
    Corrupt,
    Capacity,
    Conflict,
    NotFound,
    Io(io::Error),
}

impl fmt::Display for LocalStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => formatter.write_str("credential registry is locked"),
            Self::NotInitialized => formatter.write_str("credential registry is not initialized"),
            Self::AlreadyInitialized => formatter.write_str("credential registry is initialized"),
            Self::Corrupt => formatter.write_str("credential registry is invalid"),
            Self::Capacity => formatter.write_str("credential registry capacity reached"),
            Self::Conflict => formatter.write_str("credential command conflicts with stored state"),
            Self::NotFound => formatter.write_str("credential was not found"),
            Self::Io(error) => write!(formatter, "credential registry I/O failed: {error}"),
        }
    }
}

impl std::error::Error for LocalStoreError {}

impl From<io::Error> for LocalStoreError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug)]
pub struct BootstrapRequest {
    pub gateway_id: String,
    pub organization: OrganizationInputDto,
    pub principal: PrincipalInputDto,
    pub membership: MembershipInputDto,
    pub credential_id: String,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    pub grants: Vec<CredentialGrantDto>,
}

pub struct BootstrapOutcome {
    /// The issuance and any supersessions committed with it.
    pub transitions: Vec<CredentialTransitionDto>,
    pub metadata: CredentialMetadataDto,
    pub evidence: CredentialEvidence,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalIdentity {
    pub gateway_id: String,
    pub organization_ids: Vec<String>,
    pub revision: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredCredential {
    metadata: CredentialMetadataDto,
    verifier: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct IssueReceipt {
    issuer_principal_id: String,
    request_id: String,
    fingerprint: String,
    credential_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeReceipt {
    issuer_principal_id: String,
    request_id: String,
    credential_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Registry {
    schema_version: u32,
    gateway_id: String,
    owner_membership_id: String,
    revision: u64,
    organizations: Vec<OrganizationInputDto>,
    principals: Vec<PrincipalInputDto>,
    memberships: Vec<MembershipInputDto>,
    credentials: Vec<StoredCredential>,
    issue_receipts: Vec<IssueReceipt>,
    revoke_receipts: Vec<RevokeReceipt>,
    /// Append-only lifecycle evidence, committed with the state it describes.
    #[serde(default)]
    transitions: Vec<CredentialTransitionDto>,
}

/// One-process owner of a local registry. Opening never bootstraps credentials.
pub struct LocalCredentialStore {
    config: LocalStoreConfig,
    root: PathBuf,
    path: PathBuf,
    _lock: File,
    registry: Mutex<Option<Registry>>,
    published: RwLock<Option<Registry>>,
    subscribers: Mutex<Vec<mpsc::Sender<u64>>>,
    healthy: AtomicBool,
    #[cfg(test)]
    fail_directory_sync: AtomicBool,
    #[cfg(test)]
    fail_next_directory_sync: AtomicBool,
    #[cfg(test)]
    fail_before_replace: AtomicBool,
}

impl LocalCredentialStore {
    /// Open `path` beneath the already-private trusted `root` and acquire its
    /// sibling lifetime lock. A missing registry is represented as uninitialized.
    pub fn open(root: impl AsRef<Path>, path: impl AsRef<Path>) -> Result<Self, LocalStoreError> {
        Self::open_with_config(root, path, LocalStoreConfig::default())
    }

    pub fn open_with_config(
        root: impl AsRef<Path>,
        path: impl AsRef<Path>,
        config: LocalStoreConfig,
    ) -> Result<Self, LocalStoreError> {
        config.validate()?;
        let root = root.as_ref().to_path_buf();
        let path = path.as_ref().to_path_buf();
        let parent = path.parent().ok_or(LocalStoreError::Corrupt)?;
        set_private_directory(&root)?;
        if !parent.as_os_str().is_empty() {
            create_private_directory(&root, parent)?;
        }
        let lock_path = path.with_extension("lock");
        let lock = private_open(&root, &lock_path, true)?;
        lock.try_lock_exclusive().map_err(|error| {
            // Windows reports ERROR_LOCK_VIOLATION rather than WouldBlock.
            // Match the adapter's native contention code without hiding other I/O errors.
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                LocalStoreError::Locked
            } else {
                LocalStoreError::Io(error)
            }
        })?;
        let registry = match private_open(&root, &path, false) {
            Ok(file) => {
                let metadata = file.metadata()?;
                if metadata.len() > config.max_registry_bytes {
                    return Err(LocalStoreError::Capacity);
                }
                let mut bytes = Vec::new();
                file.take(config.max_registry_bytes + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > config.max_registry_bytes {
                    return Err(LocalStoreError::Capacity);
                }
                let mut registry: Registry =
                    serde_json::from_slice(&bytes).map_err(|_| LocalStoreError::Corrupt)?;
                upgrade_registry(&mut registry)?;
                validate_registry(&registry, &config)?;
                Some(registry)
            }
            Err(LocalStoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            config,
            root,
            path,
            _lock: lock,
            registry: Mutex::new(registry.clone()),
            published: RwLock::new(registry),
            subscribers: Mutex::new(Vec::new()),
            healthy: AtomicBool::new(true),
            #[cfg(test)]
            fail_directory_sync: AtomicBool::new(false),
            #[cfg(test)]
            fail_next_directory_sync: AtomicBool::new(false),
            #[cfg(test)]
            fail_before_replace: AtomicBool::new(false),
        })
    }

    pub fn is_initialized(&self) -> bool {
        self.registry.lock().map(|r| r.is_some()).unwrap_or(false)
    }

    pub fn gateway_id(&self) -> Result<String, LocalStoreError> {
        Ok(self
            .registry()?
            .as_ref()
            .ok_or(LocalStoreError::NotInitialized)?
            .gateway_id
            .clone())
    }

    pub fn identity(&self) -> Result<LocalIdentity, LocalStoreError> {
        let registry = self.registry()?;
        let registry = registry.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        Ok(LocalIdentity {
            gateway_id: registry.gateway_id.clone(),
            organization_ids: registry
                .organizations
                .iter()
                .map(|organization| organization.id.clone())
                .collect(),
            revision: registry.revision,
        })
    }

    /// Create the personal organization, active admin membership, and owner
    /// credential atomically. The secret is returned once and never persisted.
    pub fn bootstrap(
        &self,
        request: BootstrapRequest,
    ) -> Result<BootstrapOutcome, LocalStoreError> {
        let mut slot = self.slot()?;
        if slot.is_some() {
            return Err(LocalStoreError::AlreadyInitialized);
        }
        if request.membership.principal_id != request.principal.id
            || request.membership.organization_id != request.organization.id
            || request.membership.role != MembershipRoleDto::Admin
            || request.membership.state != MembershipStateDto::Active
        {
            return Err(LocalStoreError::Conflict);
        }
        let metadata = CredentialMetadataDto {
            id: request.credential_id,
            principal_id: request.principal.id.clone(),
            organization_id: request.organization.id.clone(),
            audience_id: request.gateway_id.clone(),
            issued_at: request.issued_at,
            expires_at: request.expires_at,
            revoked_at: None,
            grants: request.grants,
        };
        let credential = domain_credential(&metadata)?;
        validate_grants(&metadata, &request.gateway_id, true)?;
        let (evidence, verifier) = issue_secret(&metadata.id)?;
        let mut registry = Registry {
            schema_version: SCHEMA_VERSION,
            gateway_id: request.gateway_id,
            owner_membership_id: request.membership.id.clone(),
            revision: 1,
            organizations: vec![request.organization],
            principals: vec![request.principal],
            memberships: vec![request.membership],
            credentials: vec![StoredCredential {
                metadata: metadata.clone(),
                verifier,
            }],
            issue_receipts: vec![],
            revoke_receipts: vec![],
            transitions: vec![],
        };
        record_transition(
            &mut registry,
            &credential
                .issued(IssuanceCause::Bootstrap, Initiator::LocalOperator)
                .map_err(|_| LocalStoreError::Corrupt)?,
            None,
        );
        let transitions = registry.transitions.clone();
        self.persist(&registry)?;
        *slot = Some(registry.clone());
        self.publish_snapshot(registry)?;
        self.publish_revision(1);
        Ok(BootstrapOutcome {
            transitions,
            metadata,
            evidence,
            revision: 1,
        })
    }

    pub fn issue_sync(
        &self,
        request: IssueCredentialRequest,
    ) -> Result<IssueCredentialOutcome, LocalStoreError> {
        self.issue_internal(request, false)
    }

    fn issue_internal(
        &self,
        request: IssueCredentialRequest,
        administrative_allowed: bool,
    ) -> Result<IssueCredentialOutcome, LocalStoreError> {
        let fingerprint = issue_fingerprint(&request)?;
        let mut slot = self.slot()?;
        let current = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        if current.revoke_receipts.iter().any(|receipt| {
            receipt.issuer_principal_id == request.issuer_principal_id
                && receipt.request_id == request.request_id
        }) {
            return Err(LocalStoreError::Conflict);
        }
        if let Some(receipt) = current.issue_receipts.iter().find(|receipt| {
            receipt.issuer_principal_id == request.issuer_principal_id
                && receipt.request_id == request.request_id
        }) {
            if receipt.fingerprint != fingerprint {
                return Err(LocalStoreError::Conflict);
            }
            let metadata = current
                .credentials
                .iter()
                .find(|entry| entry.metadata.id == receipt.credential_id)
                .ok_or(LocalStoreError::Corrupt)?
                .metadata
                .clone();
            let transitions =
                transitions_of_command(current, &request.issuer_principal_id, &request.request_id);
            if transitions.is_empty() {
                return Err(LocalStoreError::Corrupt);
            }
            return Ok(IssueCredentialOutcome::ExistingSecretUnavailable {
                metadata,
                transitions,
            });
        }
        if current.credentials.len() >= self.config.max_credentials
            || current.issue_receipts.len() >= self.config.max_receipts
        {
            return Err(LocalStoreError::Capacity);
        }
        validate_issue(current, &request, administrative_allowed)?;
        let metadata = CredentialMetadataDto {
            id: request.credential_id.clone(),
            principal_id: request.principal.id.clone(),
            organization_id: request.membership.organization_id.clone(),
            audience_id: request.audience_id,
            issued_at: request.issued_at,
            expires_at: request.expires_at,
            revoked_at: None,
            grants: request.grants,
        };
        let credential = domain_credential(&metadata)?;
        validate_grants(&metadata, &current.gateway_id, administrative_allowed)?;
        let initiator = Initiator::Principal(
            PrincipalId::new(request.issuer_principal_id.clone())
                .map_err(|_| LocalStoreError::Conflict)?,
        );
        let (evidence, verifier) = issue_secret(&metadata.id)?;
        let mut next = current.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        let first_transition = next.transitions.len();
        if administrative_allowed {
            // Reprovisioning retires every credential of the surface principal.
            let principal_id = request.principal.id.clone();
            supersede_matching(
                &mut next,
                |existing| existing.principal_id == principal_id,
                request.issued_at,
                credential.id(),
                Supersession::Provision,
                &initiator,
                Some(&request.request_id),
            )?;
        }
        let cause = if administrative_allowed {
            IssuanceCause::SurfaceProvision
        } else {
            IssuanceCause::AdminIssue
        };
        record_transition(
            &mut next,
            &credential
                .issued(cause, initiator)
                .map_err(|_| LocalStoreError::Corrupt)?,
            Some(&request.request_id),
        );
        if !next.principals.iter().any(|p| p.id == request.principal.id) {
            next.principals.push(request.principal);
        }
        if let Some(membership) = next
            .memberships
            .iter_mut()
            .find(|m| m.id == request.membership.id)
        {
            *membership = request.membership;
        } else {
            next.memberships.push(request.membership);
        }
        next.credentials.push(StoredCredential {
            metadata: metadata.clone(),
            verifier,
        });
        next.issue_receipts.push(IssueReceipt {
            issuer_principal_id: request.issuer_principal_id,
            request_id: request.request_id,
            fingerprint,
            credential_id: metadata.id.clone(),
        });
        let transitions = next.transitions[first_transition..].to_vec();
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(IssueCredentialOutcome::Issued {
            metadata,
            evidence,
            transitions,
        })
    }

    /// Trusted offline provisioning only. The OS owner selects a distinct surface
    /// credential's exact grants; this operation is not exposed through gateway RPCs.
    pub fn provision_surface(
        &self,
        surface_id: &str,
        request_id: String,
        credential_id: String,
        actions: Vec<String>,
        issued_at: u64,
        expires_at: Option<u64>,
    ) -> Result<IssueCredentialOutcome, LocalStoreError> {
        let (mut principal, mut membership, audience_id) = {
            let slot = self.registry()?;
            let current = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
            let membership = current
                .memberships
                .iter()
                .find(|member| member.id == current.owner_membership_id)
                .ok_or(LocalStoreError::Corrupt)?
                .clone();
            let principal = current
                .principals
                .iter()
                .find(|principal| principal.id == membership.principal_id)
                .ok_or(LocalStoreError::Corrupt)?
                .clone();
            (principal, membership, current.gateway_id.clone())
        };
        if surface_id.is_empty()
            || !surface_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(LocalStoreError::Conflict);
        }
        let issuer_principal_id = principal.id.clone();
        principal.id = format!("surface:{surface_id}");
        principal.kind = PrincipalKindDto::Integration;
        membership.role = if actions.iter().any(|action| action == "credential.manage") {
            MembershipRoleDto::Admin
        } else {
            MembershipRoleDto::Member
        };
        membership.principal_id = principal.id.clone();
        membership.id = format!("surface:{surface_id}");
        let grants = actions
            .into_iter()
            .map(|action| CredentialGrantDto {
                action,
                resource: ResourceDto {
                    organization_id: membership.organization_id.clone(),
                    id: audience_id.clone(),
                },
            })
            .collect();
        self.issue_internal(
            IssueCredentialRequest {
                request_id,
                issuer_principal_id,
                credential_id,
                principal,
                membership,
                audience_id,
                issued_at,
                expires_at,
                grants,
            },
            true,
        )
    }

    /// Replace local owner credentials while holding the offline registry lock.
    /// Existing owner credentials are revoked in the same durable commit.
    pub fn recover_owner(
        &self,
        credential_id: String,
        issued_at: u64,
        expires_at: Option<u64>,
    ) -> Result<BootstrapOutcome, LocalStoreError> {
        let mut slot = self.slot()?;
        let current = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        if current.credentials.len() >= self.config.max_credentials {
            return Err(LocalStoreError::Capacity);
        }
        let membership = current
            .memberships
            .iter()
            .find(|membership| membership.id == current.owner_membership_id)
            .ok_or(LocalStoreError::Corrupt)?;
        let grants = current
            .credentials
            .iter()
            .rev()
            .find(|entry| {
                entry.metadata.principal_id == membership.principal_id
                    && entry.metadata.organization_id == membership.organization_id
                    && entry
                        .metadata
                        .grants
                        .iter()
                        .any(|grant| grant.action == "credential.manage")
            })
            .ok_or(LocalStoreError::Corrupt)?
            .metadata
            .grants
            .clone();
        let metadata = CredentialMetadataDto {
            id: credential_id,
            principal_id: membership.principal_id.clone(),
            organization_id: membership.organization_id.clone(),
            audience_id: current.gateway_id.clone(),
            issued_at,
            expires_at,
            revoked_at: None,
            grants,
        };
        let credential = domain_credential(&metadata)?;
        validate_grants(&metadata, &current.gateway_id, true)?;
        if current
            .credentials
            .iter()
            .any(|entry| entry.metadata.id == metadata.id)
        {
            return Err(LocalStoreError::Conflict);
        }
        let (evidence, verifier) = issue_secret(&metadata.id)?;
        let mut next = current.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        let first_transition = next.transitions.len();
        let (principal_id, organization_id) = (
            membership.principal_id.clone(),
            membership.organization_id.clone(),
        );
        supersede_matching(
            &mut next,
            |existing| {
                existing.principal_id == principal_id
                    && existing.organization_id == organization_id
                    && existing
                        .grants
                        .iter()
                        .any(|grant| grant.action == "credential.manage")
            },
            issued_at,
            credential.id(),
            Supersession::OwnerRecovery,
            &Initiator::LocalOperator,
            None,
        )?;
        record_transition(
            &mut next,
            &credential
                .issued(IssuanceCause::OwnerRecovery, Initiator::LocalOperator)
                .map_err(|_| LocalStoreError::Corrupt)?,
            None,
        );
        next.credentials.push(StoredCredential {
            metadata: metadata.clone(),
            verifier,
        });
        let transitions = next.transitions[first_transition..].to_vec();
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(BootstrapOutcome {
            transitions,
            metadata,
            evidence,
            revision,
        })
    }

    pub fn list_sync(
        &self,
        request: &ListCredentialsRequest,
    ) -> Result<Vec<CredentialMetadataDto>, LocalStoreError> {
        let registry = self.registry()?;
        Ok(registry
            .as_ref()
            .ok_or(LocalStoreError::NotInitialized)?
            .credentials
            .iter()
            .filter(|entry| entry.metadata.organization_id == request.organization_id)
            .map(|entry| entry.metadata.clone())
            .collect())
    }

    pub fn list_transitions_sync(
        &self,
        request: &ListTransitionsRequest,
    ) -> Result<Vec<CredentialTransitionDto>, LocalStoreError> {
        let registry = self.registry()?;
        let registry = registry.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        let owned: HashSet<&str> = registry
            .credentials
            .iter()
            .filter(|entry| entry.metadata.organization_id == request.organization_id)
            .map(|entry| entry.metadata.id.as_str())
            .collect();
        Ok(registry
            .transitions
            .iter()
            .filter(|transition| owned.contains(transition.credential_id.as_str()))
            .cloned()
            .collect())
    }

    /// Revoke once and return the revocation's evidence. A replayed command or a
    /// repeated revocation of an already-revoked credential returns the record
    /// that was committed the first time; nothing is relabelled.
    pub fn revoke_sync(
        &self,
        request: RevokeCredentialRequest,
    ) -> Result<RevokeCredentialOutcome, LocalStoreError> {
        let mut slot = self.slot()?;
        let current = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        if request.request_id.trim().is_empty() || request.request_id.len() > 200 {
            return Err(LocalStoreError::Conflict);
        }
        if current.issue_receipts.iter().any(|receipt| {
            receipt.issuer_principal_id == request.issuer_principal_id
                && receipt.request_id == request.request_id
        }) {
            return Err(LocalStoreError::Conflict);
        }
        if let Some(receipt) = current.revoke_receipts.iter().find(|receipt| {
            receipt.issuer_principal_id == request.issuer_principal_id
                && receipt.request_id == request.request_id
        }) {
            return if receipt.credential_id == request.credential_id {
                Ok(RevokeCredentialOutcome {
                    revision: current.revision,
                    revocation: revocation_of(current, &request.credential_id)
                        .ok_or(LocalStoreError::Corrupt)?,
                })
            } else {
                Err(LocalStoreError::Conflict)
            };
        }
        if !current
            .credentials
            .iter()
            .any(|entry| entry.metadata.id == request.credential_id)
        {
            return Err(LocalStoreError::NotFound);
        }
        if current.revoke_receipts.len() >= self.config.max_receipts {
            return Err(LocalStoreError::Capacity);
        }
        let initiator = Initiator::Principal(
            PrincipalId::new(request.issuer_principal_id.clone())
                .map_err(|_| LocalStoreError::Conflict)?,
        );
        let mut next = current.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        let entry = next
            .credentials
            .iter_mut()
            .find(|entry| entry.metadata.id == request.credential_id)
            .ok_or(LocalStoreError::NotFound)?;
        let mut credential = domain_credential(&entry.metadata)?;
        let transition = match credential.revoke(request.revoked_at, initiator) {
            Ok(transition) => transition,
            Err(DomainError::RevokedBeforeIssued { .. }) => return Err(LocalStoreError::Conflict),
            Err(_) => return Err(LocalStoreError::Corrupt),
        };
        entry.metadata.revoked_at = credential.revoked_at();
        if let Some(transition) = &transition {
            record_transition(&mut next, transition, Some(&request.request_id));
        }
        next.revoke_receipts.push(RevokeReceipt {
            issuer_principal_id: request.issuer_principal_id,
            request_id: request.request_id,
            credential_id: request.credential_id.clone(),
        });
        let revocation =
            revocation_of(&next, &request.credential_id).ok_or(LocalStoreError::Corrupt)?;
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(RevokeCredentialOutcome {
            revision,
            revocation,
        })
    }

    fn slot(&self) -> Result<MutexGuard<'_, Option<Registry>>, LocalStoreError> {
        if !self.healthy.load(Ordering::Acquire) {
            return Err(LocalStoreError::Corrupt);
        }
        let guard = self.registry.lock().map_err(|_| LocalStoreError::Corrupt)?;
        if !self.healthy.load(Ordering::Acquire) {
            return Err(LocalStoreError::Corrupt);
        }
        Ok(guard)
    }

    fn registry(&self) -> Result<MutexGuard<'_, Option<Registry>>, LocalStoreError> {
        let guard = self.slot()?;
        if guard.is_none() {
            return Err(LocalStoreError::NotInitialized);
        }
        Ok(guard)
    }

    fn persist(&self, registry: &Registry) -> Result<(), LocalStoreError> {
        validate_registry(registry, &self.config)?;
        let parent = self.path.parent().ok_or(LocalStoreError::Corrupt)?;
        let mut temporary = nessa_local_storage::PrivateTempFile::new_beneath(&self.root, parent)?;
        let bytes = serde_json::to_vec(registry).map_err(|_| LocalStoreError::Corrupt)?;
        if bytes.len() as u64 > self.config.max_registry_bytes {
            return Err(LocalStoreError::Capacity);
        }
        temporary.as_file_mut().write_all(&bytes)?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        #[cfg(test)]
        if self.fail_before_replace.load(Ordering::Acquire) {
            return Err(LocalStoreError::Io(io::Error::other(
                "injected pre-replace failure",
            )));
        }
        temporary
            .persist_beneath(&self.path)
            .map_err(LocalStoreError::Io)?;
        if self.sync_registry_directory(parent).is_err() {
            // A readable rename is not proof of durability. Reopen once, verify the
            // entire expected state (including receipts), then sync file + directory.
            if let Err(error) = self.reconcile_commit(&bytes, parent) {
                self.healthy.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }

    fn sync_registry_directory(&self, parent: &Path) -> Result<(), LocalStoreError> {
        #[cfg(test)]
        if self.fail_directory_sync.load(Ordering::Acquire)
            || self.fail_next_directory_sync.swap(false, Ordering::AcqRel)
        {
            return Err(LocalStoreError::Io(io::Error::other(
                "injected directory sync failure",
            )));
        }
        sync_directory_beneath(&self.root, parent)
    }

    fn reconcile_commit(&self, expected: &[u8], parent: &Path) -> Result<(), LocalStoreError> {
        let mut file = private_open(&self.root, &self.path, false)?;
        let mut actual = Vec::new();
        (&mut file)
            .take(self.config.max_registry_bytes + 1)
            .read_to_end(&mut actual)?;
        if actual != expected {
            return Err(LocalStoreError::Corrupt);
        }
        let registry: Registry =
            serde_json::from_slice(&actual).map_err(|_| LocalStoreError::Corrupt)?;
        validate_registry(&registry, &self.config)?;
        file.sync_all()?;
        self.sync_registry_directory(parent)
    }

    fn publish_revision(&self, revision: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.send(revision).is_ok());
        }
    }

    fn publish_snapshot(&self, registry: Registry) -> Result<(), LocalStoreError> {
        let mut published = self.published.write().map_err(|_| {
            self.healthy.store(false, Ordering::Release);
            LocalStoreError::Corrupt
        })?;
        *published = Some(registry);
        Ok(())
    }
}

impl CredentialVerifier for LocalCredentialStore {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            let (credential_id, secret) = parse_token(evidence.expose_bytes())?;
            if !self.healthy.load(Ordering::Acquire) {
                return Err(AccessError::Unavailable);
            }
            let registry = self
                .published
                .read()
                .map_err(|_| AccessError::Unavailable)?;
            let registry = registry.as_ref().ok_or(AccessError::Unavailable)?;
            let stored = registry
                .credentials
                .iter()
                .find(|entry| entry.metadata.id == credential_id)
                .ok_or(AccessError::InvalidCredential)?;
            if stored.metadata.audience_id != audience.as_str()
                || stored.metadata.revoked_at.is_some()
            {
                return Err(AccessError::InvalidCredential);
            }
            let expected = URL_SAFE_NO_PAD
                .decode(&stored.verifier)
                .map_err(|_| AccessError::Unavailable)?;
            let actual = Sha256::digest(&secret);
            if expected.len() != actual.len() || !bool::from(expected.ct_eq(actual.as_slice())) {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id: CredentialId::new(credential_id)
                    .map_err(|_| AccessError::Unavailable)?,
                expires_at: stored.metadata.expires_at,
            })
        })
    }
}

impl AccessReader for LocalCredentialStore {
    fn read<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            if !self.healthy.load(Ordering::Acquire) {
                return Err(AccessError::Unavailable);
            }
            let registry = self
                .published
                .read()
                .map_err(|_| AccessError::Unavailable)?;
            let registry = registry.as_ref().ok_or(AccessError::Unavailable)?;
            let stored = registry
                .credentials
                .iter()
                .find(|entry| entry.metadata.id == id.as_str())
                .ok_or(AccessError::InvalidCredential)?;
            let credential = Credential::try_from(stored.metadata.clone())
                .map_err(|_| AccessError::Unavailable)?;
            let membership = registry
                .memberships
                .iter()
                .find(|membership| {
                    membership.principal_id == stored.metadata.principal_id
                        && membership.organization_id == stored.metadata.organization_id
                })
                .cloned()
                .ok_or(AccessError::IdentityMismatch)?;
            Ok(AccessSnapshot {
                credential,
                membership: Membership::try_from(membership)
                    .map_err(|_| AccessError::Unavailable)?,
                revision: registry.revision,
            })
        })
    }
}

impl CredentialAdmin for LocalCredentialStore {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move { self.issue_sync(request).map_err(CredentialAdminError::from) })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        Box::pin(async move { self.list_sync(&request).map_err(CredentialAdminError::from) })
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            self.revoke_sync(request)
                .map_err(CredentialAdminError::from)
        })
    }
}

impl CredentialTransitionReader for LocalCredentialStore {
    fn list_transitions<'a>(
        &'a self,
        request: ListTransitionsRequest,
    ) -> PortFuture<'a, Vec<CredentialTransitionDto>, CredentialAdminError> {
        Box::pin(async move {
            self.list_transitions_sync(&request)
                .map_err(CredentialAdminError::from)
        })
    }
}

impl AuthRevisionSource for LocalCredentialStore {
    fn revision(&self) -> Result<u64, AccessError> {
        if !self.healthy.load(Ordering::Acquire) {
            return Err(AccessError::Unavailable);
        }
        self.published
            .read()
            .map_err(|_| AccessError::Unavailable)?
            .as_ref()
            .map(|registry| registry.revision)
            .ok_or(AccessError::Unavailable)
    }

    fn subscribe(&self) -> Result<mpsc::Receiver<u64>, AccessError> {
        let (sender, receiver) = mpsc::channel();
        self.subscribers
            .lock()
            .map_err(|_| AccessError::Unavailable)?
            .push(sender);
        Ok(receiver)
    }
}

/// Write one-time evidence to a new private file and durably sync it. Existing
/// paths are never overwritten. The caller should revoke if this returns an error
/// after registry issuance committed.
pub fn write_evidence_file(
    path: impl AsRef<Path>,
    evidence: &CredentialEvidence,
) -> Result<(), LocalStoreError> {
    let path = path.as_ref();
    let mut file = nessa_local_storage::open(path, nessa_local_storage::OpenMode::CreateNew)?;
    file.write_all(evidence.expose_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_all()?;
    nessa_local_storage::sync_directory(path.parent().ok_or(LocalStoreError::Corrupt)?)?;
    Ok(())
}

impl From<LocalStoreError> for CredentialAdminError {
    fn from(error: LocalStoreError) -> Self {
        match error {
            LocalStoreError::Conflict => Self::Conflict,
            LocalStoreError::Capacity => Self::Capacity,
            LocalStoreError::NotFound => Self::NotFound,
            LocalStoreError::Locked
            | LocalStoreError::NotInitialized
            | LocalStoreError::AlreadyInitialized
            | LocalStoreError::Corrupt
            | LocalStoreError::Io(_) => Self::Unavailable,
        }
    }
}

fn issue_secret(credential_id: &str) -> Result<(CredentialEvidence, String), LocalStoreError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|_| LocalStoreError::Corrupt)?;
    let encoded_id = URL_SAFE_NO_PAD.encode(credential_id.as_bytes());
    let token = format!(
        "{TOKEN_PREFIX}.{encoded_id}.{}",
        URL_SAFE_NO_PAD.encode(secret)
    );
    let verifier = URL_SAFE_NO_PAD.encode(Sha256::digest(secret));
    let evidence =
        CredentialEvidence::new(token.into_bytes()).map_err(|_| LocalStoreError::Corrupt)?;
    Ok((evidence, verifier))
}

fn parse_token(bytes: &[u8]) -> Result<(String, Vec<u8>), AccessError> {
    let token = std::str::from_utf8(bytes).map_err(|_| AccessError::InvalidCredential)?;
    let mut pieces = token.split('.');
    if pieces.next() != Some(TOKEN_PREFIX) {
        return Err(AccessError::InvalidCredential);
    }
    let id = pieces.next().ok_or(AccessError::InvalidCredential)?;
    let secret = pieces.next().ok_or(AccessError::InvalidCredential)?;
    if pieces.next().is_some() {
        return Err(AccessError::InvalidCredential);
    }
    let id = URL_SAFE_NO_PAD
        .decode(id)
        .map_err(|_| AccessError::InvalidCredential)?;
    let id = String::from_utf8(id).map_err(|_| AccessError::InvalidCredential)?;
    let secret = URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(|_| AccessError::InvalidCredential)?;
    if secret.len() != 32 {
        return Err(AccessError::InvalidCredential);
    }
    Ok((id, secret))
}

/// Rebuild the domain credential a stored record describes. A record the
/// domain rejects is a conflict on the way in and corruption on the way out.
fn domain_credential(metadata: &CredentialMetadataDto) -> Result<Credential, LocalStoreError> {
    Credential::try_from(metadata.clone()).map_err(|_| LocalStoreError::Conflict)
}

fn validate_metadata(metadata: &CredentialMetadataDto) -> Result<(), LocalStoreError> {
    domain_credential(metadata).map(|_| ())
}

/// Append domain evidence under the revision `registry` is about to commit.
/// Callers bump `registry.revision` before recording.
fn record_transition(
    registry: &mut Registry,
    transition: &CredentialTransition,
    correlation: Option<&str>,
) {
    let sequence = registry.transitions.len() as u64 + 1;
    registry.transitions.push(CredentialTransitionDto::record(
        transition,
        sequence,
        registry.revision,
        correlation.map(str::to_owned),
    ));
}

/// Retire every credential `retire` selects in favour of `by`, through the
/// domain's supersession rule, recording one transition per credential that
/// actually changed. Already-revoked credentials keep their original record.
fn supersede_matching(
    registry: &mut Registry,
    retire: impl Fn(&CredentialMetadataDto) -> bool,
    at: u64,
    by: &CredentialId,
    kind: Supersession,
    initiator: &Initiator,
    correlation: Option<&str>,
) -> Result<(), LocalStoreError> {
    let mut transitions = Vec::new();
    for entry in registry
        .credentials
        .iter_mut()
        .filter(|entry| retire(&entry.metadata))
    {
        let mut credential =
            Credential::try_from(entry.metadata.clone()).map_err(|_| LocalStoreError::Corrupt)?;
        if let Some(transition) = credential.supersede(at, by.clone(), kind, initiator.clone()) {
            entry.metadata.revoked_at = credential.revoked_at();
            transitions.push(transition);
        }
    }
    for transition in &transitions {
        record_transition(registry, transition, correlation);
    }
    Ok(())
}

/// The record under which `credential_id` stopped being valid: its revocation
/// transition, or a pre-journal record that already showed it revoked.
fn revocation_of(registry: &Registry, credential_id: &str) -> Option<CredentialTransitionDto> {
    registry
        .transitions
        .iter()
        .rev()
        .find(|transition| {
            transition.credential_id == credential_id && transition.after.revoked_at.is_some()
        })
        .cloned()
}

/// Every transition one issue command committed, identified by its initiator
/// and idempotency key.
fn transitions_of_command(
    registry: &Registry,
    issuer_principal_id: &str,
    request_id: &str,
) -> Vec<CredentialTransitionDto> {
    registry
        .transitions
        .iter()
        .filter(|transition| {
            transition.correlation.as_deref() == Some(request_id)
                && matches!(&transition.initiator, InitiatorDto::Principal { id } if id == issuer_principal_id)
        })
        .cloned()
        .collect()
}

/// Bring a schema 1 registry into the current shape in memory. Credentials that
/// existed before transitions were recorded get one honest `predates_journal`
/// record each; their real cause is unknown and is not guessed.
fn upgrade_registry(registry: &mut Registry) -> Result<(), LocalStoreError> {
    if registry.schema_version != LEGACY_SCHEMA_VERSION {
        return Ok(());
    }
    if !registry.transitions.is_empty() {
        return Err(LocalStoreError::Corrupt);
    }
    for index in 0..registry.credentials.len() {
        let credential = Credential::try_from(registry.credentials[index].metadata.clone())
            .map_err(|_| LocalStoreError::Corrupt)?;
        let transition = CredentialTransition::new(
            credential.id().clone(),
            None,
            credential.lifecycle(),
            TransitionCause::PredatesJournal,
            Initiator::Unknown,
            credential.issued_at(),
        )
        .map_err(|_| LocalStoreError::Corrupt)?;
        record_transition(registry, &transition, None);
    }
    registry.schema_version = SCHEMA_VERSION;
    Ok(())
}

/// The one rule for recorded evidence, applied before every write and on every
/// reopen: each transition satisfies the domain, chains from the previous state
/// of its credential, and ends at the state the registry actually stores.
fn validate_transitions(
    registry: &Registry,
    config: &LocalStoreConfig,
    principals: &HashSet<&String>,
) -> Result<(), LocalStoreError> {
    if registry.transitions.len() > 2 * config.max_credentials {
        return Err(LocalStoreError::Corrupt);
    }
    let mut chains: HashMap<&str, Vec<&CredentialTransitionDto>> = HashMap::new();
    let mut previous_revision = 0;
    for (index, transition) in registry.transitions.iter().enumerate() {
        if transition.sequence != index as u64 + 1
            || transition.revision < previous_revision
            || transition.revision > registry.revision
            || transition
                .correlation
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 200)
        {
            return Err(LocalStoreError::Corrupt);
        }
        previous_revision = transition.revision;
        CredentialTransition::try_from(transition.clone()).map_err(|_| LocalStoreError::Corrupt)?;
        if let InitiatorDto::Principal { id } = &transition.initiator {
            if !principals.contains(id) {
                return Err(LocalStoreError::Corrupt);
            }
        }
        chains
            .entry(transition.credential_id.as_str())
            .or_default()
            .push(transition);
    }
    let issued_revision = |credential_id: &str| {
        chains
            .get(credential_id)
            .and_then(|chain| chain.first())
            .map(|first| first.revision)
    };
    let mut seen = 0;
    for stored in &registry.credentials {
        let chain = chains
            .get(stored.metadata.id.as_str())
            .ok_or(LocalStoreError::Corrupt)?;
        seen += chain.len();
        let (first, rest) = chain.split_first().ok_or(LocalStoreError::Corrupt)?;
        if !matches!(
            first.cause,
            TransitionCauseDto::Issued { .. } | TransitionCauseDto::PredatesJournal
        ) || rest.len() > 1
        {
            return Err(LocalStoreError::Corrupt);
        }
        let mut previous = first;
        for transition in rest {
            let TransitionCauseDto::Revoked { cause } = &transition.cause else {
                return Err(LocalStoreError::Corrupt);
            };
            if transition.before != Some(previous.after) {
                return Err(LocalStoreError::Corrupt);
            }
            if let crate::application::dto::RevocationCauseDto::Superseded { by, .. } = cause {
                if issued_revision(by) != Some(transition.revision) {
                    return Err(LocalStoreError::Corrupt);
                }
            }
            previous = transition;
        }
        if previous.after.issued_at != stored.metadata.issued_at
            || previous.after.expires_at != stored.metadata.expires_at
            || previous.after.revoked_at != stored.metadata.revoked_at
        {
            return Err(LocalStoreError::Corrupt);
        }
    }
    // A transition naming a credential the registry does not hold.
    if seen != registry.transitions.len() {
        return Err(LocalStoreError::Corrupt);
    }
    Ok(())
}

fn validate_registry(
    registry: &Registry,
    config: &LocalStoreConfig,
) -> Result<(), LocalStoreError> {
    if registry.schema_version != SCHEMA_VERSION
        || registry.credentials.len() > config.max_credentials
        || registry.issue_receipts.len() > config.max_receipts
        || registry.revoke_receipts.len() > config.max_receipts
        || registry.gateway_id.is_empty()
        || !registry.memberships.iter().any(|membership| {
            membership.id == registry.owner_membership_id
                && membership.role == MembershipRoleDto::Admin
                && membership.state == MembershipStateDto::Active
        })
    {
        return Err(LocalStoreError::Corrupt);
    }
    let organizations: HashSet<_> = registry
        .organizations
        .iter()
        .map(|value| &value.id)
        .collect();
    let principals: HashSet<_> = registry.principals.iter().map(|value| &value.id).collect();
    validate_transitions(registry, config, &principals)?;
    let memberships: HashSet<_> = registry.memberships.iter().map(|value| &value.id).collect();
    let membership_bindings: HashSet<_> = registry
        .memberships
        .iter()
        .map(|value| (&value.principal_id, &value.organization_id))
        .collect();
    let credentials: HashSet<_> = registry
        .credentials
        .iter()
        .map(|value| &value.metadata.id)
        .collect();
    if organizations.len() != registry.organizations.len()
        || principals.len() != registry.principals.len()
        || memberships.len() != registry.memberships.len()
        || membership_bindings.len() != registry.memberships.len()
        || credentials.len() != registry.credentials.len()
    {
        return Err(LocalStoreError::Corrupt);
    }
    for membership in &registry.memberships {
        if !principals.contains(&membership.principal_id)
            || !organizations.contains(&membership.organization_id)
            || Membership::try_from(membership.clone()).is_err()
        {
            return Err(LocalStoreError::Corrupt);
        }
    }
    for credential in &registry.credentials {
        validate_metadata(&credential.metadata).map_err(|_| LocalStoreError::Corrupt)?;
        validate_grants(&credential.metadata, &registry.gateway_id, true)
            .map_err(|_| LocalStoreError::Corrupt)?;
        if credential.metadata.audience_id != registry.gateway_id
            || !principals.contains(&credential.metadata.principal_id)
            || !organizations.contains(&credential.metadata.organization_id)
            || !registry.memberships.iter().any(|membership| {
                membership.principal_id == credential.metadata.principal_id
                    && membership.organization_id == credential.metadata.organization_id
            })
        {
            return Err(LocalStoreError::Corrupt);
        }
        let verifier = URL_SAFE_NO_PAD
            .decode(&credential.verifier)
            .map_err(|_| LocalStoreError::Corrupt)?;
        if verifier.len() != 32 {
            return Err(LocalStoreError::Corrupt);
        }
    }
    let issue_commands: HashSet<_> = registry
        .issue_receipts
        .iter()
        .map(|receipt| (&receipt.issuer_principal_id, &receipt.request_id))
        .collect();
    let revoke_commands: HashSet<_> = registry
        .revoke_receipts
        .iter()
        .map(|receipt| (&receipt.issuer_principal_id, &receipt.request_id))
        .collect();
    if issue_commands.len() != registry.issue_receipts.len()
        || revoke_commands.len() != registry.revoke_receipts.len()
        || registry.issue_receipts.iter().any(|receipt| {
            !principals.contains(&receipt.issuer_principal_id)
                || !credentials.contains(&receipt.credential_id)
        })
        || registry.revoke_receipts.iter().any(|receipt| {
            !principals.contains(&receipt.issuer_principal_id)
                || !credentials.contains(&receipt.credential_id)
        })
    {
        return Err(LocalStoreError::Corrupt);
    }
    Ok(())
}

fn validate_issue(
    registry: &Registry,
    request: &IssueCredentialRequest,
    administrative_allowed: bool,
) -> Result<(), LocalStoreError> {
    if request.audience_id != registry.gateway_id
        || request.request_id.trim().is_empty()
        || request.request_id.len() > 200
        || request.issuer_principal_id.trim().is_empty()
        || !registry
            .principals
            .iter()
            .any(|principal| principal.id == request.issuer_principal_id)
        || !registry
            .organizations
            .iter()
            .any(|organization| organization.id == request.membership.organization_id)
        || request.membership.principal_id != request.principal.id
        || (!administrative_allowed && request.membership.role == MembershipRoleDto::Admin)
        || request.membership.state != MembershipStateDto::Active
        || request
            .grants
            .iter()
            .any(|grant| grant.resource.organization_id != request.membership.organization_id)
        || registry
            .credentials
            .iter()
            .any(|entry| entry.metadata.id == request.credential_id)
    {
        return Err(LocalStoreError::Conflict);
    }
    if let Some(principal) = registry
        .principals
        .iter()
        .find(|p| p.id == request.principal.id)
    {
        if principal != &request.principal {
            return Err(LocalStoreError::Conflict);
        }
    }
    if let Some(membership) = registry
        .memberships
        .iter()
        .find(|m| m.id == request.membership.id)
    {
        if membership != &request.membership
            && !(administrative_allowed
                && membership.id != registry.owner_membership_id
                && membership.principal_id == request.membership.principal_id
                && membership.organization_id == request.membership.organization_id)
        {
            return Err(LocalStoreError::Conflict);
        }
    }
    if registry.memberships.iter().any(|membership| {
        membership.principal_id == request.membership.principal_id
            && membership.organization_id == request.membership.organization_id
            && membership.id != request.membership.id
    }) {
        return Err(LocalStoreError::Conflict);
    }
    Ok(())
}

fn validate_grants(
    metadata: &CredentialMetadataDto,
    gateway_id: &str,
    administrative_allowed: bool,
) -> Result<(), LocalStoreError> {
    if metadata.grants.is_empty()
        || metadata.grants.iter().any(|grant| {
            grant.resource.id != gateway_id
                || !matches!(
                    grant.action.as_str(),
                    "server.read" | "credential.manage" | "conversation.write"
                )
                || (!administrative_allowed && grant.action == "credential.manage")
        })
    {
        return Err(LocalStoreError::Conflict);
    }
    Ok(())
}

fn issue_fingerprint(request: &IssueCredentialRequest) -> Result<String, LocalStoreError> {
    let value = serde_json::json!({
        "principal": request.principal,
        "membership": request.membership,
        "audienceId": request.audience_id,
        "expiresAt": request.expires_at,
        "grants": request.grants,
    });
    let bytes = serde_json::to_vec(&value).map_err(|_| LocalStoreError::Corrupt)?;
    Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)))
}

fn private_open(root: &Path, path: &Path, create: bool) -> Result<File, LocalStoreError> {
    nessa_local_storage::open_beneath(
        root,
        path,
        if create {
            nessa_local_storage::OpenMode::OpenOrCreate
        } else {
            nessa_local_storage::OpenMode::ReadWrite
        },
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::PermissionDenied
            || matches!(
                error.raw_os_error(),
                Some(libc::ELOOP) | Some(libc::ENOTDIR)
            )
        {
            LocalStoreError::Corrupt
        } else {
            LocalStoreError::Io(error)
        }
    })
}
fn set_private_directory(path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::verify_directory(path)?)
}
fn create_private_directory(root: &Path, path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::create_directory_beneath(root, path)?)
}
fn sync_directory_beneath(root: &Path, path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::sync_directory_beneath(root, path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::MembershipRole;
    use crate::{
        application::dto::{
            CredentialGrantDto, CredentialLifecycleDto, IssuanceCauseDto, PrincipalKindDto,
            ResourceDto, RevocationCauseDto, SupersessionDto,
        },
        domain::AudienceId,
    };
    use std::{
        fs,
        future::Future,
        task::{Context, Poll, Waker},
    };

    fn ready<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::noop();
        match std::pin::pin!(future).poll(&mut Context::from_waker(waker)) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("local adapter unexpectedly yielded"),
        }
    }

    fn trusted_root(path: &Path) -> (&Path, &Path) {
        let root = path.parent().unwrap();
        if fs::symlink_metadata(root).is_err() {
            nessa_local_storage::create_directory(root).unwrap();
        }
        (root, path.strip_prefix(root).unwrap())
    }

    fn open_store(path: impl AsRef<Path>) -> Result<LocalCredentialStore, LocalStoreError> {
        let path = path.as_ref();
        let (root, relative) = trusted_root(path);
        LocalCredentialStore::open(root, relative)
    }

    fn open_store_with_config(
        path: impl AsRef<Path>,
        config: LocalStoreConfig,
    ) -> Result<LocalCredentialStore, LocalStoreError> {
        let path = path.as_ref();
        let (root, relative) = trusted_root(path);
        LocalCredentialStore::open_with_config(root, relative, config)
    }

    #[cfg(windows)]
    #[test]
    fn inherited_temp_root_is_rejected_while_a_protected_child_is_accepted() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(LocalCredentialStore::open(temporary.path(), "credentials.v1.json").is_err());

        let trusted = temporary.path().join("trusted");
        nessa_local_storage::create_directory(&trusted).unwrap();
        assert!(LocalCredentialStore::open(&trusted, "credentials.v1.json").is_ok());
    }

    fn grant(org: &str, action: &str) -> CredentialGrantDto {
        CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: org.into(),
                id: "gateway-1".into(),
            },
        }
    }

    fn bootstrap() -> BootstrapRequest {
        BootstrapRequest {
            gateway_id: "gateway-1".into(),
            organization: OrganizationInputDto { id: "org-1".into() },
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "owner-membership".into(),
                principal_id: "owner".into(),
                organization_id: "org-1".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: "owner-credential".into(),
            issued_at: 100,
            expires_at: Some(200),
            grants: vec![grant("org-1", "credential.manage")],
        }
    }

    #[test]
    fn configured_limits_apply_to_mutations_reopen_and_independent_stores() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("limited/credentials.v1.json");
        let small = LocalStoreConfig {
            max_credentials: 1,
            ..LocalStoreConfig::default()
        };
        let larger = LocalStoreConfig {
            max_credentials: 2,
            ..small
        };
        let store = open_store_with_config(&path, small).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        assert!(matches!(
            store.recover_owner("next".into(), 110, None),
            Err(LocalStoreError::Capacity)
        ));
        drop(store);
        let store = open_store_with_config(&path, larger).unwrap();
        store.recover_owner("next".into(), 110, None).unwrap();
        drop(store);
        assert!(open_store_with_config(&path, small).is_err());
        assert!(open_store_with_config(&path, larger).is_ok());
        let independent = open_store(root.path().join("other/credentials.v1.json")).unwrap();
        independent.bootstrap(bootstrap()).unwrap();
        independent.recover_owner("next".into(), 110, None).unwrap();
        let tiny = LocalStoreConfig {
            max_registry_bytes: 16,
            ..LocalStoreConfig::default()
        };
        let tiny_store =
            open_store_with_config(root.path().join("tiny/credentials.v1.json"), tiny).unwrap();
        assert!(matches!(
            tiny_store.bootstrap(bootstrap()),
            Err(LocalStoreError::Capacity)
        ));
        assert!(!tiny_store.is_initialized());
    }

    #[test]
    fn bootstrap_is_explicit_private_and_exclusively_locked() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        assert!(!store.is_initialized());
        let outcome = store.bootstrap(bootstrap()).unwrap();
        let error = match open_store(&path) {
            Ok(_) => panic!("a second store acquired the exclusive registry lock"),
            Err(error) => error,
        };
        assert!(
            matches!(error, LocalStoreError::Locked),
            "unexpected lock error: {error:?}"
        );
        let json = fs::read_to_string(&path).unwrap();
        let token = std::str::from_utf8(outcome.evidence.expose_bytes()).unwrap();
        assert!(!json.contains(token));
        assert!(!json.contains("nessa_v1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn verification_and_revocation_survive_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let outcome = store.bootstrap(bootstrap()).unwrap();
        let token = outcome.evidence.expose_bytes().to_vec();
        let audience = AudienceId::new("gateway-1").unwrap();
        assert!(
            ready(store.verify(&CredentialEvidence::new(token.clone()).unwrap(), &audience))
                .is_ok()
        );
        store
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-1".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "owner-credential".into(),
                revoked_at: 150,
            })
            .unwrap();
        drop(store);
        let reopened = open_store(&path).unwrap();
        assert_eq!(
            ready(reopened.verify(&CredentialEvidence::new(token).unwrap(), &audience)),
            Err(AccessError::InvalidCredential)
        );
    }

    #[test]
    fn authentication_reads_prior_snapshot_while_mutation_is_in_progress() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let outcome = store.bootstrap(bootstrap()).unwrap();
        let mutation = store.registry.lock().unwrap();

        assert!(
            ready(store.verify(&outcome.evidence, &AudienceId::new("gateway-1").unwrap())).is_ok()
        );
        drop(mutation);
    }

    #[test]
    fn issue_is_idempotent_without_replaying_secret_and_notifies_revision() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let receiver = store.subscribe().unwrap();
        let request = IssueCredentialRequest {
            request_id: "issue-1".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "reader-credential".into(),
            principal: PrincipalInputDto {
                id: "reader".into(),
                kind: PrincipalKindDto::Agent,
            },
            membership: MembershipInputDto {
                id: "reader-membership".into(),
                principal_id: "reader".into(),
                organization_id: "org-1".into(),
                role: MembershipRoleDto::Member,
                state: MembershipStateDto::Active,
            },
            audience_id: "gateway-1".into(),
            issued_at: 110,
            expires_at: Some(190),
            grants: vec![grant("org-1", "server.read")],
        };
        assert!(matches!(
            store.issue_sync(request.clone()).unwrap(),
            IssueCredentialOutcome::Issued { .. }
        ));
        assert_eq!(receiver.recv().unwrap(), 2);
        assert!(matches!(
            store.issue_sync(request.clone()).unwrap(),
            IssueCredentialOutcome::ExistingSecretUnavailable { .. }
        ));
        let mut changed = request.clone();
        changed.expires_at = changed.expires_at.map(|expiry| expiry + 1);
        assert!(matches!(
            store.issue_sync(changed),
            Err(LocalStoreError::Conflict)
        ));
        assert!(matches!(
            store.revoke_sync(RevokeCredentialRequest {
                request_id: request.request_id,
                issuer_principal_id: request.issuer_principal_id,
                credential_id: request.credential_id,
                revoked_at: 150
            }),
            Err(LocalStoreError::Conflict)
        ));
    }

    #[test]
    fn surface_credentials_are_distinct_nonexpiring_and_reprovisioning_revokes_only_that_surface() {
        let root = tempfile::tempdir().unwrap();
        let store = open_store(root.path().join("auth/credentials.v1.json")).unwrap();
        let owner = store.bootstrap(bootstrap()).unwrap();
        let issue = |surface: &str, id: &str, grants: Vec<String>| {
            store
                .provision_surface(
                    surface,
                    format!("request-{id}"),
                    id.into(),
                    grants,
                    150,
                    None,
                )
                .unwrap()
        };
        let IssueCredentialOutcome::Issued {
            metadata,
            evidence: chat,
            ..
        } = issue(
            "nessa-panel",
            "chat-one",
            vec![
                "server.read".into(),
                "conversation.write".into(),
                "credential.manage".into(),
            ],
        )
        else {
            panic!("new credential")
        };
        assert_eq!(metadata.expires_at, None);
        assert_eq!(metadata.principal_id, "surface:nessa-panel");
        let IssueCredentialOutcome::Issued {
            evidence: plugin, ..
        } = issue("plugin", "plugin-one", vec!["server.read".into()])
        else {
            panic!("new credential")
        };
        let roles = || {
            let slot = store.registry().unwrap();
            let registry = slot.as_ref().unwrap();
            assert!(registry
                .principals
                .iter()
                .filter(|principal| principal.id.starts_with("surface:"))
                .all(|principal| principal.kind == PrincipalKindDto::Integration));
            registry
                .memberships
                .iter()
                .filter(|membership| membership.id.starts_with("surface:"))
                .map(|membership| (membership.id.clone(), membership.role.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            roles(),
            vec![
                ("surface:nessa-panel".into(), MembershipRoleDto::Admin),
                ("surface:plugin".into(), MembershipRoleDto::Member),
            ]
        );
        issue("nessa-panel", "chat-two", vec!["server.read".into()]);
        assert!(roles()
            .iter()
            .all(|(_, role)| *role == MembershipRoleDto::Member));
        // Role and revocations are published coherently to active readers.
        let snapshot = ready(store.read(&CredentialId::new("chat-two").unwrap())).unwrap();
        assert_eq!(snapshot.membership.role(), MembershipRole::Member);

        let audience = AudienceId::new("gateway-1").unwrap();
        assert_eq!(
            ready(store.verify(&chat, &audience)),
            Err(AccessError::InvalidCredential)
        );
        assert!(ready(store.verify(&plugin, &audience)).is_ok());
        assert!(ready(store.verify(&owner.evidence, &audience)).is_ok());
    }

    #[test]
    fn owner_recovery_preserves_identity_and_revokes_previous_owner() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let original = store.bootstrap(bootstrap()).unwrap();
        let original_token = original.evidence.expose_bytes().to_vec();
        store
            .provision_surface(
                "panel",
                "panel-issue".into(),
                "panel-token".into(),
                vec!["credential.manage".into()],
                110,
                None,
            )
            .unwrap();
        drop(store);
        let mut registry: Registry = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        registry.memberships.reverse();
        registry.principals.reverse();
        fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
        let store = open_store(&path).unwrap();

        let identity = store.identity().unwrap();
        let recovered = store
            .recover_owner("replacement-owner".into(), 150, Some(250))
            .unwrap();
        assert_eq!(recovered.metadata.principal_id, "owner");
        assert_eq!(store.identity().unwrap().gateway_id, identity.gateway_id);
        assert_eq!(
            store.identity().unwrap().organization_ids,
            identity.organization_ids
        );
        let audience = AudienceId::new("gateway-1").unwrap();
        assert_eq!(
            ready(store.verify(&CredentialEvidence::new(original_token).unwrap(), &audience)),
            Err(AccessError::InvalidCredential)
        );
        assert!(ready(store.verify(&recovered.evidence, &audience)).is_ok());
    }

    #[test]
    fn reprovisioning_behind_an_existing_issuance_still_revokes_it() {
        // `issued_at` is supplied by the caller, so a replacement can carry a
        // timestamp behind the credential it supersedes. Recording a revocation
        // before its own issuance is what the domain refuses, and this registry
        // validates every stored credential on the way to disk — so the whole
        // reprovisioning used to fail as corrupt instead of revoking anything.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let first = store
            .provision_surface(
                "panel",
                "panel-1".into(),
                "panel-token-1".into(),
                vec!["server.read".into()],
                300,
                None,
            )
            .unwrap();
        let IssueCredentialOutcome::Issued { evidence, .. } = first else {
            panic!("a new surface credential was expected")
        };

        let replacement = store
            .provision_surface(
                "panel",
                "panel-2".into(),
                "panel-token-2".into(),
                vec!["server.read".into()],
                200,
                None,
            )
            .expect("reprovisioning must not fail because of the earlier timestamp");
        let IssueCredentialOutcome::Issued {
            evidence: replacement_evidence,
            ..
        } = replacement
        else {
            panic!("a new surface credential was expected")
        };

        let audience = AudienceId::new("gateway-1").unwrap();
        assert_eq!(
            ready(store.verify(&evidence, &audience)),
            Err(AccessError::InvalidCredential)
        );
        assert!(ready(store.verify(&replacement_evidence, &audience)).is_ok());
        drop(store);

        // The revocation is never recorded before the credential it ends, so the
        // registry it wrote is one that can be read back.
        let registry: Registry = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let superseded = registry
            .credentials
            .iter()
            .find(|entry| entry.metadata.id == "panel-token-1")
            .expect("the superseded credential is retained");
        assert_eq!(superseded.metadata.revoked_at, Some(300));
        assert!(open_store(&path).is_ok());

        // A third provisioning does not move the revocation already recorded:
        // the first one is when the credential stopped being usable.
        let store = open_store(&path).unwrap();
        store
            .provision_surface(
                "panel",
                "panel-3".into(),
                "panel-token-3".into(),
                vec!["server.read".into()],
                900,
                None,
            )
            .unwrap();
        drop(store);
        let registry: Registry = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let superseded = registry
            .credentials
            .iter()
            .find(|entry| entry.metadata.id == "panel-token-1")
            .expect("the superseded credential is retained");
        assert_eq!(superseded.metadata.revoked_at, Some(300));
    }

    #[test]
    fn empty_grants_and_receipt_capacity_fail_without_changing_durable_state() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store_with_config(
            &path,
            LocalStoreConfig {
                max_receipts: 1,
                ..LocalStoreConfig::default()
            },
        )
        .unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            store.provision_surface("empty", "empty".into(), "empty".into(), vec![], 110, None),
            Err(LocalStoreError::Conflict)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        store
            .provision_surface(
                "one",
                "one".into(),
                "one".into(),
                vec!["server.read".into()],
                110,
                None,
            )
            .unwrap();
        let full = fs::read(&path).unwrap();
        assert!(matches!(
            store.provision_surface(
                "two",
                "two".into(),
                "two".into(),
                vec!["server.read".into()],
                110,
                None
            ),
            Err(LocalStoreError::Capacity)
        ));
        // An exact retry still resolves its receipt at capacity.
        assert!(matches!(
            store.provision_surface(
                "one",
                "one".into(),
                "one".into(),
                vec!["server.read".into()],
                110,
                None
            ),
            Ok(IssueCredentialOutcome::ExistingSecretUnavailable { .. })
        ));
        assert_eq!(fs::read(&path).unwrap(), full);
        drop(store);
        assert!(open_store(&path).is_ok());
    }

    #[test]
    fn administration_preserves_rejected_command_errors_through_the_port() {
        let root = tempfile::tempdir().unwrap();
        let store = open_store(root.path().join("auth/credentials.v1.json")).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let request = RevokeCredentialRequest {
            request_id: "revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "missing".into(),
            revoked_at: 120,
        };
        assert_eq!(
            ready(store.revoke(request.clone())),
            Err(CredentialAdminError::NotFound)
        );
        let mut request = RevokeCredentialRequest {
            credential_id: "owner-credential".into(),
            ..request
        };
        assert!(ready(store.revoke(request.clone())).is_ok());
        request.credential_id = "different".into();
        assert_eq!(
            ready(store.revoke(request)),
            Err(CredentialAdminError::Conflict)
        );
    }

    #[test]
    fn pre_replace_failure_keeps_prior_auth_and_allows_same_request_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let owner = store.bootstrap(bootstrap()).unwrap();
        let before = fs::read(&path).unwrap();
        let request = RevokeCredentialRequest {
            request_id: "retry-once".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "owner-credential".into(),
            revoked_at: 150,
        };
        store.fail_before_replace.store(true, Ordering::Release);
        assert!(store.revoke_sync(request.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(
            ready(store.verify(&owner.evidence, &AudienceId::new("gateway-1").unwrap())).is_ok()
        );
        store.fail_before_replace.store(false, Ordering::Release);
        assert_eq!(store.revoke_sync(request.clone()).unwrap().revision, 2);
        assert_eq!(store.revoke_sync(request).unwrap().revision, 2);
    }

    #[test]
    fn one_failed_sync_reconciles_without_reapplying_mutation_and_survives_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let owner = store.bootstrap(bootstrap()).unwrap();
        let revisions = store.subscribe().unwrap();
        let request = RevokeCredentialRequest {
            request_id: "reconciled".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "owner-credential".into(),
            revoked_at: 150,
        };
        store
            .fail_next_directory_sync
            .store(true, Ordering::Release);
        assert_eq!(store.revoke_sync(request.clone()).unwrap().revision, 2);
        assert_eq!(revisions.recv().unwrap(), 2);
        assert_eq!(store.revoke_sync(request.clone()).unwrap().revision, 2);
        assert!(revisions.try_recv().is_err());
        assert_eq!(
            ready(store.verify(&owner.evidence, &AudienceId::new("gateway-1").unwrap())),
            Err(AccessError::InvalidCredential)
        );
        drop(store);
        let reopened = open_store(&path).unwrap();
        assert_eq!(reopened.revoke_sync(request).unwrap().revision, 2);
    }

    #[test]
    fn reconciliation_rejects_unexpected_bytes_even_if_they_are_valid_json() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        assert!(matches!(
            store.reconcile_commit(b"{}", path.parent().unwrap()),
            Err(LocalStoreError::Corrupt)
        ));
    }

    #[test]
    fn post_replace_sync_uncertainty_latches_store_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let original = store.bootstrap(bootstrap()).unwrap();
        store.fail_directory_sync.store(true, Ordering::Release);
        assert!(store
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-uncertain".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "owner-credential".into(),
                revoked_at: 150,
            })
            .is_err());
        assert_eq!(
            ready(store.verify(&original.evidence, &AudienceId::new("gateway-1").unwrap())),
            Err(AccessError::Unavailable)
        );
        drop(store);
        let reopened = open_store(&path).unwrap();
        assert_eq!(
            ready(reopened.verify(&original.evidence, &AudienceId::new("gateway-1").unwrap())),
            Err(AccessError::InvalidCredential)
        );
    }

    #[test]
    fn malformed_registry_fails_instead_of_resetting_identity() {
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth");
        nessa_local_storage::create_directory(&auth).unwrap();
        nessa_local_storage::open(
            &auth.join("credentials.v1.json"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap()
        .write_all(b"{}")
        .unwrap();
        assert!(matches!(
            open_store(auth.join("credentials.v1.json")),
            Err(LocalStoreError::Corrupt)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_registry_and_directory_symlinks_are_not_adopted() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth");
        fs::create_dir(&auth).unwrap();
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o700)).unwrap();
        let registry = auth.join("credentials.v1.json");
        symlink(root.path().join("missing.json"), &registry).unwrap();
        assert!(open_store(&registry).is_err());
        assert!(fs::symlink_metadata(&registry)
            .unwrap()
            .file_type()
            .is_symlink());

        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("linked-auth");
        symlink(&target, &link).unwrap();
        assert!(open_store(link.join("credentials.v1.json")).is_err());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );

        let outside = tempfile::tempdir().unwrap();
        let redirected = outside.path().join("redirected");
        fs::create_dir(&redirected).unwrap();
        fs::set_permissions(&redirected, fs::Permissions::from_mode(0o700)).unwrap();
        let intermediate = root.path().join("nested-link");
        symlink(&redirected, &intermediate).unwrap();
        assert!(LocalCredentialStore::open(
            root.path(),
            Path::new("nested-link/credentials.v1.json")
        )
        .is_err());
        assert!(!redirected.join("credentials.lock").exists());
        assert!(!redirected.join("credentials.v1.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn shared_registry_files_are_rejected_without_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        drop(store);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(open_store(&path).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&path, root.path().join("shared.json")).unwrap();
        assert!(open_store(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn registry_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth");
        nessa_local_storage::create_directory(&auth).unwrap();
        let target = root.path().join("target.json");
        fs::write(&target, b"{}").unwrap();
        let registry = auth.join("credentials.v1.json");
        symlink(target, &registry).unwrap();
        assert!(matches!(
            open_store(registry),
            Err(LocalStoreError::Corrupt)
        ));
    }

    // ---- lifecycle transition evidence ----

    fn transitions(store: &LocalCredentialStore) -> Vec<CredentialTransitionDto> {
        store
            .list_transitions_sync(&ListTransitionsRequest {
                organization_id: "org-1".into(),
            })
            .unwrap()
    }
    fn member_issue(request_id: &str, credential_id: &str) -> IssueCredentialRequest {
        IssueCredentialRequest {
            request_id: request_id.into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential_id.into(),
            principal: PrincipalInputDto {
                id: "reader".into(),
                kind: PrincipalKindDto::Integration,
            },
            membership: MembershipInputDto {
                id: "reader-membership".into(),
                principal_id: "reader".into(),
                organization_id: "org-1".into(),
                role: MembershipRoleDto::Member,
                state: MembershipStateDto::Active,
            },
            audience_id: "gateway-1".into(),
            issued_at: 120,
            expires_at: None,
            grants: vec![grant("org-1", "server.read")],
        }
    }
    fn revoke(request_id: &str, credential_id: &str, at: u64) -> RevokeCredentialRequest {
        RevokeCredentialRequest {
            request_id: request_id.into(),
            issuer_principal_id: "owner".into(),
            credential_id: credential_id.into(),
            revoked_at: at,
        }
    }
    fn principal(id: &str) -> InitiatorDto {
        InitiatorDto::Principal { id: id.into() }
    }
    fn lifecycle(
        issued_at: u64,
        expires_at: Option<u64>,
        revoked_at: Option<u64>,
    ) -> CredentialLifecycleDto {
        CredentialLifecycleDto {
            issued_at,
            expires_at,
            revoked_at,
        }
    }
    type Edit = Box<dyn FnOnce(&mut serde_json::Value)>;
    fn rewrite(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        edit(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn every_lifecycle_path_records_target_before_after_cause_and_initiator() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();

        let booted = store.bootstrap(bootstrap()).unwrap();
        assert_eq!(booted.transitions, transitions(&store));
        let issued = store
            .issue_sync(member_issue("issue-reader", "reader-credential"))
            .unwrap();
        let IssueCredentialOutcome::Issued {
            transitions: issued,
            ..
        } = issued
        else {
            panic!("fresh issuance")
        };
        assert_eq!(issued.len(), 1);
        store
            .provision_surface(
                "panel",
                "panel-1".into(),
                "panel-a".into(),
                vec!["server.read".into()],
                130,
                None,
            )
            .unwrap();
        let reprovisioned = store
            .provision_surface(
                "panel",
                "panel-2".into(),
                "panel-b".into(),
                vec!["server.read".into()],
                140,
                None,
            )
            .unwrap();
        let IssueCredentialOutcome::Issued {
            transitions: reprovisioned,
            ..
        } = reprovisioned
        else {
            panic!("fresh reprovisioning")
        };
        let revoked = store
            .revoke_sync(revoke("revoke-reader", "reader-credential", 150))
            .unwrap();
        let recovered = store.recover_owner("owner-2".into(), 160, None).unwrap();

        let all = transitions(&store);
        assert_eq!(
            all.iter().map(|t| t.sequence).collect::<Vec<_>>(),
            (1..=all.len() as u64).collect::<Vec<_>>()
        );
        let expected = vec![
            CredentialTransitionDto {
                sequence: 1,
                revision: 1,
                correlation: None,
                credential_id: "owner-credential".into(),
                before: None,
                after: lifecycle(100, Some(200), None),
                cause: TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::Bootstrap,
                },
                initiator: InitiatorDto::LocalOperator,
                at: 100,
            },
            CredentialTransitionDto {
                sequence: 2,
                revision: 2,
                correlation: Some("issue-reader".into()),
                credential_id: "reader-credential".into(),
                before: None,
                after: lifecycle(120, None, None),
                cause: TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::AdminIssue,
                },
                initiator: principal("owner"),
                at: 120,
            },
            CredentialTransitionDto {
                sequence: 3,
                revision: 3,
                correlation: Some("panel-1".into()),
                credential_id: "panel-a".into(),
                before: None,
                after: lifecycle(130, None, None),
                cause: TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::SurfaceProvision,
                },
                initiator: principal("owner"),
                at: 130,
            },
            CredentialTransitionDto {
                sequence: 4,
                revision: 4,
                correlation: Some("panel-2".into()),
                credential_id: "panel-a".into(),
                before: Some(lifecycle(130, None, None)),
                after: lifecycle(130, None, Some(140)),
                cause: TransitionCauseDto::Revoked {
                    cause: RevocationCauseDto::Superseded {
                        by: "panel-b".into(),
                        supersession: SupersessionDto::Provision,
                    },
                },
                initiator: principal("owner"),
                at: 140,
            },
            CredentialTransitionDto {
                sequence: 5,
                revision: 4,
                correlation: Some("panel-2".into()),
                credential_id: "panel-b".into(),
                before: None,
                after: lifecycle(140, None, None),
                cause: TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::SurfaceProvision,
                },
                initiator: principal("owner"),
                at: 140,
            },
            CredentialTransitionDto {
                sequence: 6,
                revision: 5,
                correlation: Some("revoke-reader".into()),
                credential_id: "reader-credential".into(),
                before: Some(lifecycle(120, None, None)),
                after: lifecycle(120, None, Some(150)),
                cause: TransitionCauseDto::Revoked {
                    cause: RevocationCauseDto::Explicit,
                },
                initiator: principal("owner"),
                at: 150,
            },
            CredentialTransitionDto {
                sequence: 7,
                revision: 6,
                correlation: None,
                credential_id: "owner-credential".into(),
                before: Some(lifecycle(100, Some(200), None)),
                after: lifecycle(100, Some(200), Some(160)),
                cause: TransitionCauseDto::Revoked {
                    cause: RevocationCauseDto::Superseded {
                        by: "owner-2".into(),
                        supersession: SupersessionDto::OwnerRecovery,
                    },
                },
                initiator: InitiatorDto::LocalOperator,
                at: 160,
            },
            CredentialTransitionDto {
                sequence: 8,
                revision: 6,
                correlation: None,
                credential_id: "owner-2".into(),
                before: None,
                after: lifecycle(160, None, None),
                cause: TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::OwnerRecovery,
                },
                initiator: InitiatorDto::LocalOperator,
                at: 160,
            },
        ];
        assert_eq!(all, expected);
        assert_eq!(issued, expected[1..2]);
        assert_eq!(reprovisioned, expected[3..5]);
        assert_eq!(revoked.revocation, expected[5]);
        assert_eq!(recovered.transitions, expected[6..8]);
        assert_eq!(
            ready(store.list_transitions(ListTransitionsRequest {
                organization_id: "elsewhere".into()
            }))
            .unwrap(),
            Vec::new()
        );

        drop(store);
        assert_eq!(transitions(&open_store(&path).unwrap()), expected);
    }

    #[test]
    fn a_credential_is_revoked_once_and_keeps_its_first_cause() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        store
            .provision_surface(
                "panel",
                "panel-1".into(),
                "panel-a".into(),
                vec!["server.read".into()],
                130,
                None,
            )
            .unwrap();
        store
            .provision_surface(
                "panel",
                "panel-2".into(),
                "panel-b".into(),
                vec!["server.read".into()],
                140,
                None,
            )
            .unwrap();
        let superseded =
            revocation_of(store.registry().unwrap().as_ref().unwrap(), "panel-a").unwrap();

        // An explicit revoke after supersession changes nothing and reports the
        // supersession, not a relabelled explicit revocation.
        let again = store.revoke_sync(revoke("late", "panel-a", 170)).unwrap();
        assert_eq!(again.revocation, superseded);
        let replayed = store.revoke_sync(revoke("late", "panel-a", 999)).unwrap();
        assert_eq!(replayed, again);
        let other_key = store.revoke_sync(revoke("later", "panel-a", 180)).unwrap();
        assert_eq!(other_key.revocation, superseded);
        assert_eq!(
            transitions(&store)
                .iter()
                .filter(|t| t.credential_id == "panel-a" && t.before.is_some())
                .count(),
            1
        );

        // A deliberate time before issuance is the caller's mistake and leaves no trace.
        let before = transitions(&store);
        assert!(matches!(
            store.revoke_sync(revoke("early", "panel-b", 139)),
            Err(LocalStoreError::Conflict)
        ));
        assert_eq!(transitions(&store), before);
    }

    #[test]
    fn idempotent_issue_replay_returns_the_original_transitions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let first = store
            .issue_sync(member_issue("issue-reader", "reader-credential"))
            .unwrap();
        let IssueCredentialOutcome::Issued {
            transitions: original,
            ..
        } = first
        else {
            panic!("fresh issuance")
        };
        let replay = store
            .issue_sync(member_issue("issue-reader", "reader-credential"))
            .unwrap();
        let IssueCredentialOutcome::ExistingSecretUnavailable {
            transitions: replayed,
            ..
        } = replay
        else {
            panic!("replay returns no secret")
        };
        assert_eq!(replayed, original);
        assert_eq!(transitions(&store).len(), 2);
    }

    #[test]
    fn a_failed_commit_leaves_no_evidence_and_the_retry_records_it_once() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        let request = revoke("revoke-owner", "owner-credential", 150);

        store.fail_before_replace.store(true, Ordering::Release);
        assert!(store.revoke_sync(request.clone()).is_err());
        store.fail_before_replace.store(false, Ordering::Release);
        assert_eq!(transitions(&store).len(), 1);
        drop(store);
        let store = open_store(&path).unwrap();
        assert_eq!(transitions(&store).len(), 1);

        store.fail_directory_sync.store(true, Ordering::Release);
        assert!(store.revoke_sync(request.clone()).is_err());
        drop(store);
        let store = open_store(&path).unwrap();
        // The replaced file is either the old or the new state; both are consistent.
        let recorded = transitions(&store);
        assert!(recorded.len() <= 2);
        let outcome = store.revoke_sync(request).unwrap();
        let recorded = transitions(&store);
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1], outcome.revocation);
        assert_eq!(
            recorded[1].cause,
            TransitionCauseDto::Revoked {
                cause: RevocationCauseDto::Explicit
            }
        );
    }

    #[test]
    fn tampered_or_missing_evidence_is_rejected_by_the_same_rule_on_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        store
            .issue_sync(member_issue("issue-reader", "reader-credential"))
            .unwrap();
        store
            .revoke_sync(revoke("revoke-reader", "reader-credential", 150))
            .unwrap();
        drop(store);
        let good = fs::read(&path).unwrap();
        assert!(open_store(&path).is_ok());

        let cases: Vec<(&str, Edit)> = vec![
            (
                "revoked credential without its revocation record",
                Box::new(|v| {
                    v["transitions"].as_array_mut().unwrap().pop();
                }),
            ),
            (
                "gap in sequence",
                Box::new(|v| {
                    v["transitions"][2]["sequence"] = serde_json::json!(4);
                }),
            ),
            (
                "before does not chain from the prior state",
                Box::new(|v| {
                    v["transitions"][2]["before"]["expiresAt"] = serde_json::json!(999);
                }),
            ),
            (
                "unknown initiator on a live record",
                Box::new(|v| {
                    v["transitions"][1]["initiator"] = serde_json::json!({"kind": "unknown"});
                }),
            ),
            (
                "initiator names no principal",
                Box::new(|v| {
                    v["transitions"][1]["initiator"] =
                        serde_json::json!({"kind": "principal", "id": "ghost"});
                }),
            ),
            (
                "record for a credential the registry does not hold",
                Box::new(|v| {
                    v["transitions"][1]["credentialId"] = serde_json::json!("ghost-credential");
                }),
            ),
            (
                "revocation time disagrees with the credential",
                Box::new(|v| {
                    v["credentials"][1]["metadata"]["revokedAt"] = serde_json::json!(151);
                }),
            ),
            (
                "revision after the registry revision",
                Box::new(|v| {
                    v["transitions"][2]["revision"] = serde_json::json!(99);
                }),
            ),
            (
                "relabelled cause with the wrong shape",
                Box::new(|v| {
                    v["transitions"][2]["cause"] =
                        serde_json::json!({"kind": "issued", "cause": "admin_issue"});
                }),
            ),
        ];
        for (name, edit) in cases {
            fs::write(&path, &good).unwrap();
            rewrite(&path, edit);
            assert!(
                matches!(open_store(&path), Err(LocalStoreError::Corrupt)),
                "accepted: {name}"
            );
        }
    }

    #[test]
    fn schema_one_registry_gets_honest_pre_journal_records_and_upgrades_on_first_write() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        store
            .issue_sync(member_issue("issue-reader", "reader-credential"))
            .unwrap();
        store
            .revoke_sync(revoke("revoke-reader", "reader-credential", 150))
            .unwrap();
        drop(store);
        rewrite(&path, |v| {
            v["schemaVersion"] = serde_json::json!(1);
            v.as_object_mut().unwrap().remove("transitions");
        });
        let legacy = fs::read(&path).unwrap();

        let store = open_store(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), legacy, "open never writes");
        let backfilled = transitions(&store);
        assert_eq!(
            backfilled,
            vec![
                CredentialTransitionDto {
                    sequence: 1,
                    revision: 3,
                    correlation: None,
                    credential_id: "owner-credential".into(),
                    before: None,
                    after: lifecycle(100, Some(200), None),
                    cause: TransitionCauseDto::PredatesJournal,
                    initiator: InitiatorDto::Unknown,
                    at: 100,
                },
                CredentialTransitionDto {
                    sequence: 2,
                    revision: 3,
                    correlation: None,
                    credential_id: "reader-credential".into(),
                    before: None,
                    after: lifecycle(120, None, Some(150)),
                    cause: TransitionCauseDto::PredatesJournal,
                    initiator: InitiatorDto::Unknown,
                    at: 120,
                },
            ]
        );
        // A pre-journal revocation is still the credential's one revocation.
        let replay = store
            .revoke_sync(revoke("again", "reader-credential", 170))
            .unwrap();
        assert_eq!(replay.revocation, backfilled[1]);

        let outcome = store
            .revoke_sync(revoke("revoke-owner", "owner-credential", 180))
            .unwrap();
        let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["schemaVersion"], 2);
        assert_eq!(written["transitions"].as_array().unwrap().len(), 3);
        assert_eq!(
            outcome.revocation.before,
            Some(lifecycle(100, Some(200), None))
        );
        drop(store);
        assert_eq!(transitions(&open_store(&path).unwrap()).len(), 3);

        // A schema 1 file that already claims transitions is not trusted.
        fs::write(&path, &legacy).unwrap();
        rewrite(&path, |v| {
            v["transitions"] = serde_json::json!([]);
        });
        assert!(open_store(&path).is_ok());
        rewrite(&path, |v| {
            v["transitions"] = serde_json::json!([{
                "sequence": 1, "revision": 1, "correlation": null,
                "credentialId": "owner-credential", "before": null,
                "after": {"issuedAt": 100, "expiresAt": 200, "revokedAt": null},
                "cause": {"kind": "issued", "cause": "bootstrap"},
                "initiator": {"kind": "local_operator"}, "at": 100
            }]);
        });
        assert!(matches!(open_store(&path), Err(LocalStoreError::Corrupt)));
    }
}
