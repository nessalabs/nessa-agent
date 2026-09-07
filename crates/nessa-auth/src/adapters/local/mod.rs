//! Durable local credential registry and bearer-token verifier.
//!
//! The registry stores only SHA-256 verifiers for 256-bit random secrets. SHA-256
//! is appropriate here because secrets are uniformly random, not user passwords;
//! verification uses a constant-time comparison. The registry lock is held for
//! this store's lifetime and every mutation is persisted before publication.

use crate::{
    application::{
        credential_admin::{
            AuthRevisionSource, CredentialAdmin, IssueCredentialOutcome, IssueCredentialRequest,
            ListCredentialsRequest, RevokeCredentialRequest,
        },
        dto::{
            CredentialGrantDto, CredentialMetadataDto, MembershipInputDto, MembershipRoleDto,
            MembershipStateDto, OrganizationInputDto, PrincipalInputDto,
        },
        ports::{
            AccessError, AccessReader, AccessSnapshot, CredentialEvidence, CredentialVerifier,
            PortFuture, VerifiedCredential,
        },
    },
    domain::{AudienceId, Credential, CredentialId, Membership},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex, MutexGuard, RwLock,
    },
};
use subtle::ConstantTimeEq;

const SCHEMA_VERSION: u32 = 1;
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
    revision: u64,
    organizations: Vec<OrganizationInputDto>,
    principals: Vec<PrincipalInputDto>,
    memberships: Vec<MembershipInputDto>,
    credentials: Vec<StoredCredential>,
    issue_receipts: Vec<IssueReceipt>,
    revoke_receipts: Vec<RevokeReceipt>,
}

/// One-process owner of a local registry. Opening never bootstraps credentials.
pub struct LocalCredentialStore {
    config: LocalStoreConfig,
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
    /// Open `path` and acquire its sibling lifetime lock. A missing registry is
    /// represented as uninitialized so only an explicit offline bootstrap creates it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LocalStoreError> {
        Self::open_with_config(path, LocalStoreConfig::default())
    }

    pub fn open_with_config(
        path: impl AsRef<Path>,
        config: LocalStoreConfig,
    ) -> Result<Self, LocalStoreError> {
        config.validate()?;
        let path = path.as_ref().to_path_buf();
        let parent = path.parent().ok_or(LocalStoreError::Corrupt)?;
        create_private_directory(parent)?;
        set_private_directory(parent)?;
        if fs::symlink_metadata(parent)?.file_type().is_symlink() {
            return Err(LocalStoreError::Corrupt);
        }
        let lock_path = path.with_extension("lock");
        let lock = private_open(&lock_path, true)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                LocalStoreError::Locked
            } else {
                LocalStoreError::Io(error)
            }
        })?;
        let registry = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(LocalStoreError::Corrupt);
                }
                if metadata.len() > config.max_registry_bytes {
                    return Err(LocalStoreError::Capacity);
                }
                let mut bytes = Vec::new();
                private_open(&path, false)?
                    .take(config.max_registry_bytes + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > config.max_registry_bytes {
                    return Err(LocalStoreError::Capacity);
                }
                let registry: Registry =
                    serde_json::from_slice(&bytes).map_err(|_| LocalStoreError::Corrupt)?;
                validate_registry(&registry, &config)?;
                Some(registry)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            config,
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
        validate_metadata(&metadata)?;
        validate_grants(&metadata, &request.gateway_id, true)?;
        let (evidence, verifier) = issue_secret(&metadata.id)?;
        let registry = Registry {
            schema_version: SCHEMA_VERSION,
            gateway_id: request.gateway_id,
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
        };
        self.persist(&registry)?;
        *slot = Some(registry.clone());
        self.publish_snapshot(registry)?;
        self.publish_revision(1);
        Ok(BootstrapOutcome {
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
            return Ok(IssueCredentialOutcome::ExistingSecretUnavailable { metadata });
        }
        if current.credentials.len() >= self.config.max_credentials {
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
        validate_metadata(&metadata)?;
        validate_grants(&metadata, &current.gateway_id, administrative_allowed)?;
        let (evidence, verifier) = issue_secret(&metadata.id)?;
        let mut next = current.clone();
        if administrative_allowed {
            for entry in &mut next.credentials {
                if entry.metadata.principal_id == request.principal.id
                    && entry.metadata.revoked_at.is_none()
                {
                    entry.metadata.revoked_at = Some(request.issued_at);
                }
            }
        }
        if !next.principals.iter().any(|p| p.id == request.principal.id) {
            next.principals.push(request.principal);
        }
        if !next
            .memberships
            .iter()
            .any(|m| m.id == request.membership.id)
        {
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
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(IssueCredentialOutcome::Issued { metadata, evidence })
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
                .find(|member| {
                    member.role == MembershipRoleDto::Admin
                        && member.state == MembershipStateDto::Active
                })
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
        membership.principal_id = principal.id.clone();
        membership.id = format!("surface:{surface_id}");
        let grants = actions
            .into_iter()
            .map(|action| CredentialGrantDto {
                action,
                resource: crate::application::dto::ResourceDto {
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
            .find(|membership| {
                membership.role == MembershipRoleDto::Admin
                    && membership.state == MembershipStateDto::Active
            })
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
        validate_metadata(&metadata)?;
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
        for entry in &mut next.credentials {
            if entry.metadata.principal_id == membership.principal_id
                && entry.metadata.organization_id == membership.organization_id
                && entry
                    .metadata
                    .grants
                    .iter()
                    .any(|grant| grant.action == "credential.manage")
                && entry.metadata.revoked_at.is_none()
            {
                entry.metadata.revoked_at = Some(issued_at.max(entry.metadata.issued_at));
            }
        }
        next.credentials.push(StoredCredential {
            metadata: metadata.clone(),
            verifier,
        });
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(BootstrapOutcome {
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

    pub fn revoke_sync(&self, request: RevokeCredentialRequest) -> Result<u64, LocalStoreError> {
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
                Ok(current.revision)
            } else {
                Err(LocalStoreError::Conflict)
            };
        }
        let mut next = current.clone();
        let credential = next
            .credentials
            .iter_mut()
            .find(|entry| entry.metadata.id == request.credential_id)
            .ok_or(LocalStoreError::NotFound)?;
        if next.revoke_receipts.len() >= self.config.max_receipts {
            return Err(LocalStoreError::Capacity);
        }
        if request.revoked_at < credential.metadata.issued_at {
            return Err(LocalStoreError::Conflict);
        }
        credential
            .metadata
            .revoked_at
            .get_or_insert(request.revoked_at);
        next.revoke_receipts.push(RevokeReceipt {
            issuer_principal_id: request.issuer_principal_id,
            request_id: request.request_id,
            credential_id: request.credential_id,
        });
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(revision)
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
        let mut temporary = nessa_local_storage::PrivateTempFile::new_in(parent)?;
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
        temporary.persist(&self.path).map_err(LocalStoreError::Io)?;
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
        sync_directory(parent)
    }

    fn reconcile_commit(&self, expected: &[u8], parent: &Path) -> Result<(), LocalStoreError> {
        let mut file = private_open(&self.path, false)?;
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
    ) -> PortFuture<'a, IssueCredentialOutcome> {
        Box::pin(async move { self.issue_sync(request).map_err(map_error) })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>> {
        Box::pin(async move { self.list_sync(&request).map_err(map_error) })
    }
    fn revoke<'a>(&'a self, request: RevokeCredentialRequest) -> PortFuture<'a, u64> {
        Box::pin(async move { self.revoke_sync(request).map_err(map_error) })
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
    sync_directory(path.parent().ok_or(LocalStoreError::Corrupt)?)?;
    Ok(())
}

fn map_error(_: LocalStoreError) -> AccessError {
    AccessError::Unavailable
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

fn validate_metadata(metadata: &CredentialMetadataDto) -> Result<(), LocalStoreError> {
    Credential::try_from(metadata.clone())
        .map(|_| ())
        .map_err(|_| LocalStoreError::Conflict)
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
    {
        return Err(LocalStoreError::Corrupt);
    }
    let organizations: HashSet<_> = registry
        .organizations
        .iter()
        .map(|value| &value.id)
        .collect();
    let principals: HashSet<_> = registry.principals.iter().map(|value| &value.id).collect();
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
        if membership != &request.membership {
            return Err(LocalStoreError::Conflict);
        }
    }
    if registry.memberships.iter().any(|membership| {
        membership.principal_id == request.membership.principal_id
            && membership.organization_id == request.membership.organization_id
            && membership != &request.membership
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
    if metadata.grants.iter().any(|grant| {
        grant.resource.id != gateway_id
            || !matches!(
                grant.action.as_str(),
                "server.read" | "credential.manage" | "conversation.write"
            )
            || (!administrative_allowed && grant.action == "credential.manage")
    }) {
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

fn private_open(path: &Path, create: bool) -> Result<File, LocalStoreError> {
    nessa_local_storage::open(
        path,
        if create {
            nessa_local_storage::OpenMode::OpenOrCreate
        } else {
            nessa_local_storage::OpenMode::ReadWrite
        },
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::PermissionDenied {
            LocalStoreError::Corrupt
        } else {
            LocalStoreError::Io(error)
        }
    })
}
fn set_private_directory(path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::verify_directory(path)?)
}
fn create_private_directory(path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::create_directory(path)?)
}
fn sync_directory(path: &Path) -> Result<(), LocalStoreError> {
    Ok(nessa_local_storage::sync_directory(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::dto::{CredentialGrantDto, PrincipalKindDto, ResourceDto},
        domain::AudienceId,
    };
    use std::{
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
        let store = LocalCredentialStore::open_with_config(&path, small).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        assert!(matches!(
            store.recover_owner("next".into(), 110, None),
            Err(LocalStoreError::Capacity)
        ));
        drop(store);
        let store = LocalCredentialStore::open_with_config(&path, larger).unwrap();
        store.recover_owner("next".into(), 110, None).unwrap();
        drop(store);
        assert!(LocalCredentialStore::open_with_config(&path, small).is_err());
        assert!(LocalCredentialStore::open_with_config(&path, larger).is_ok());
        let independent =
            LocalCredentialStore::open(root.path().join("other/credentials.v1.json")).unwrap();
        independent.bootstrap(bootstrap()).unwrap();
        independent.recover_owner("next".into(), 110, None).unwrap();
        let tiny = LocalStoreConfig {
            max_registry_bytes: 16,
            ..LocalStoreConfig::default()
        };
        let tiny_store = LocalCredentialStore::open_with_config(
            root.path().join("tiny/credentials.v1.json"),
            tiny,
        )
        .unwrap();
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
        let store = LocalCredentialStore::open(&path).unwrap();
        assert!(!store.is_initialized());
        let outcome = store.bootstrap(bootstrap()).unwrap();
        assert!(matches!(
            LocalCredentialStore::open(&path),
            Err(LocalStoreError::Locked)
        ));
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
        let store = LocalCredentialStore::open(&path).unwrap();
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
        let reopened = LocalCredentialStore::open(&path).unwrap();
        assert_eq!(
            ready(reopened.verify(&CredentialEvidence::new(token).unwrap(), &audience)),
            Err(AccessError::InvalidCredential)
        );
    }

    #[test]
    fn authentication_reads_prior_snapshot_while_mutation_is_in_progress() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = LocalCredentialStore::open(&path).unwrap();
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
        let store = LocalCredentialStore::open(&path).unwrap();
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
        let store =
            LocalCredentialStore::open(root.path().join("auth/credentials.v1.json")).unwrap();
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
        issue("nessa-panel", "chat-two", vec!["server.read".into()]);
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
        let store = LocalCredentialStore::open(&path).unwrap();
        let original = store.bootstrap(bootstrap()).unwrap();
        let original_token = original.evidence.expose_bytes().to_vec();
        let identity = store.identity().unwrap();
        let recovered = store
            .recover_owner("replacement-owner".into(), 150, Some(250))
            .unwrap();
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
    fn pre_replace_failure_keeps_prior_auth_and_allows_same_request_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = LocalCredentialStore::open(&path).unwrap();
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
        assert_eq!(store.revoke_sync(request.clone()).unwrap(), 2);
        assert_eq!(store.revoke_sync(request).unwrap(), 2);
    }

    #[test]
    fn one_failed_sync_reconciles_without_reapplying_mutation_and_survives_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = LocalCredentialStore::open(&path).unwrap();
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
        assert_eq!(store.revoke_sync(request.clone()).unwrap(), 2);
        assert_eq!(revisions.recv().unwrap(), 2);
        assert_eq!(store.revoke_sync(request.clone()).unwrap(), 2);
        assert!(revisions.try_recv().is_err());
        assert_eq!(
            ready(store.verify(&owner.evidence, &AudienceId::new("gateway-1").unwrap())),
            Err(AccessError::InvalidCredential)
        );
        drop(store);
        let reopened = LocalCredentialStore::open(&path).unwrap();
        assert_eq!(reopened.revoke_sync(request).unwrap(), 2);
    }

    #[test]
    fn reconciliation_rejects_unexpected_bytes_even_if_they_are_valid_json() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = LocalCredentialStore::open(&path).unwrap();
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
        let store = LocalCredentialStore::open(&path).unwrap();
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
        let reopened = LocalCredentialStore::open(&path).unwrap();
        assert_eq!(
            ready(reopened.verify(&original.evidence, &AudienceId::new("gateway-1").unwrap())),
            Err(AccessError::InvalidCredential)
        );
    }

    #[test]
    fn malformed_registry_fails_instead_of_resetting_identity() {
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth");
        create_private_directory(&auth).unwrap();
        nessa_local_storage::open(
            &auth.join("credentials.v1.json"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap()
        .write_all(b"{}")
        .unwrap();
        assert!(matches!(
            LocalCredentialStore::open(auth.join("credentials.v1.json")),
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
        let registry = auth.join("credentials.v1.json");
        symlink(root.path().join("missing.json"), &registry).unwrap();
        assert!(LocalCredentialStore::open(&registry).is_err());
        assert!(fs::symlink_metadata(&registry)
            .unwrap()
            .file_type()
            .is_symlink());

        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("linked-auth");
        symlink(&target, &link).unwrap();
        assert!(LocalCredentialStore::open(link.join("credentials.v1.json")).is_err());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn shared_registry_files_are_rejected_without_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = LocalCredentialStore::open(&path).unwrap();
        store.bootstrap(bootstrap()).unwrap();
        drop(store);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(LocalCredentialStore::open(&path).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&path, root.path().join("shared.json")).unwrap();
        assert!(LocalCredentialStore::open(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn registry_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth");
        fs::create_dir(&auth).unwrap();
        let target = root.path().join("target.json");
        fs::write(&target, b"{}").unwrap();
        let registry = auth.join("credentials.v1.json");
        symlink(target, &registry).unwrap();
        assert!(matches!(
            LocalCredentialStore::open(registry),
            Err(LocalStoreError::Corrupt)
        ));
    }
}
