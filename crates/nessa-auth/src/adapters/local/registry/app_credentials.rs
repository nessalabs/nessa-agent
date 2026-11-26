//! Durable app consent, owned by the same transaction as credential issuance.
//! Ordinary Product proof and session entry do not admit this issuance.

use super::{
    access_snapshot, check_secret, issue_fingerprint, parse_token, LocalCredentialStore,
    LocalStoreError, Registry, StoredCredential, StoredProof,
};
use crate::{
    application::{
        app_credential::{
            AppCapabilities, AppCapability, AppCredentialBinding, AppCredentialSnapshot,
            AppCredentials, ApprovedAppIssue, IssueAppCredential,
        },
        credential_admin::{
            CredentialIssuePurpose, IssueCredentialOutcome, IssueCredentialRequest,
        },
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::{
            AccessError, AccessSnapshot, Clock, CredentialEvidence, Decision, IssuePath,
            PolicyEvaluator, PortFuture,
        },
        session::{AuthenticatedSession, ReadCurrentSession},
    },
    domain::{
        Action, AudienceId, CredentialId, DomainError, MembershipId, OrganizationId, PrincipalId,
        Resource, ResourceId,
    },
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nessa_types::app_identity::ExtensionAppId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum AppProof {
    AppBearer {
        verifier: String,
        scope: StoredAppScope,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct StoredAppScope {
    app_id: String,
    operations: Vec<AppCapability>,
}

pub(super) struct AppIssueCommit<'a> {
    pub(super) clock: &'a dyn Clock,
    pub(super) approval_deadline: Option<u64>,
    pub(super) scope: StoredAppScope,
    pub(super) observed_revision: u64,
}

impl LocalCredentialStore {
    /// Commit the app and exact operations selected by an authenticated owner.
    /// This is a local auth operation, not a Product method. The approved
    /// credential is not accepted by the ordinary Product verifier.
    pub async fn issue_app(
        &self,
        session: &AuthenticatedSession,
        approval: IssueAppCredential,
        clock: &dyn Clock,
        policy: &dyn PolicyEvaluator,
    ) -> Result<IssueCredentialOutcome, LocalStoreError> {
        let snapshot = ReadCurrentSession {
            access: self,
            clock,
        }
        .execute(session)
        .await
        .map_err(|_| LocalStoreError::Conflict)?;
        let context = session.context();
        let owner_membership = self
            .with_published_registry(|registry| {
                if registry.revision != snapshot.revision {
                    return Err(AccessError::StaleRevision);
                }
                MembershipId::new(&registry.owner_membership_id)
                    .map_err(|_| AccessError::Unavailable)
            })
            .map_err(|_| LocalStoreError::Conflict)?;
        let action = Action::new("appCredential.issue").map_err(|_| LocalStoreError::Corrupt)?;
        let resource = Resource::new(
            context.organization_id().clone(),
            ResourceId::new(context.audience_id().as_str())
                .map_err(|_| LocalStoreError::Corrupt)?,
        )
        .with_owner_membership(owner_membership);
        if policy.evaluate(context, &action, &resource, &snapshot) != Ok(Decision::Allow) {
            return Err(LocalStoreError::Conflict);
        }
        let now = clock.unix_seconds();
        if now >= approval.expires_at {
            return Err(LocalStoreError::Conflict);
        }
        let principal = app_principal(
            context.principal_id(),
            context.organization_id(),
            context.audience_id(),
            &approval.app,
        )
        .map_err(|_| LocalStoreError::Corrupt)?
        .as_str()
        .to_owned();
        let organization = context.organization_id().as_str();
        let audience = context.audience_id().as_str();
        self.commit_issue(
            IssueCredentialRequest {
                purpose: CredentialIssuePurpose::Product,
                request_id: approval.request_id,
                issuer_principal_id: context.principal_id().as_str().into(),
                credential_id: approval.credential_id.as_str().into(),
                principal: PrincipalInputDto {
                    id: principal.clone(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: principal.clone(),
                    principal_id: principal,
                    organization_id: organization.into(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: audience.into(),
                issued_at: now,
                expires_at: Some(approval.expires_at),
                // Only identity is provisioned in this slice; scoped operations
                // are the consent record, not generic conversation grants.
                grants: vec![CredentialGrantDto {
                    action: "app.identity".into(),
                    resource: ResourceDto {
                        organization_id: organization.into(),
                        id: audience.into(),
                    },
                }],
            },
            IssuePath::AppIssue,
            Some(AppIssueCommit {
                clock,
                approval_deadline: [session.expires_at(), snapshot.credential.expires_at()]
                    .into_iter()
                    .flatten()
                    .min(),
                scope: StoredAppScope {
                    app_id: approval.app.as_str().into(),
                    operations: approval.capabilities.iter().collect(),
                },
                observed_revision: snapshot.revision,
            }),
        )
    }

    /// Read a current app binding from the same snapshot as its issued scope and
    /// receipt. This is secret-free registry inspection, not session admission.
    pub fn app_binding(
        &self,
        id: &CredentialId,
        now: u64,
    ) -> Result<AppCredentialBinding, AccessError> {
        self.with_published_registry(|registry| {
            let stored = registry
                .credentials
                .iter()
                .find(|entry| entry.metadata.id == id.as_str())
                .ok_or(AccessError::InvalidCredential)?;
            let snapshot = access_snapshot(registry, stored)?;
            let binding = approved_app_binding(registry, stored, &snapshot)?;
            if snapshot.credential.revoked_at().is_some() {
                return Err(AccessError::CredentialRevoked);
            }
            if snapshot
                .credential
                .expires_at()
                .is_some_and(|expiry| now >= expiry)
            {
                return Err(AccessError::CredentialExpired);
            }
            if !snapshot.credential.is_valid_at(now) {
                return Err(AccessError::InvalidCredential);
            }
            if !snapshot.membership.is_active() {
                return Err(AccessError::InactiveMembership);
            }
            Ok(binding)
        })
    }
}

impl AppCredentials for LocalCredentialStore {
    fn verify_app<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, AppCredentialSnapshot> {
        Box::pin(async move {
            let (credential_id, secret) = parse_token(evidence.expose_bytes())?;
            self.with_published_registry(|registry| {
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
                let StoredProof::App(AppProof::AppBearer { verifier, .. }) = &stored.verifier
                else {
                    return Err(AccessError::InvalidCredential);
                };
                check_secret(verifier, &secret)?;
                scoped_snapshot(registry, stored)
            })
        })
    }

    fn read_app<'a>(&'a self, id: &'a CredentialId) -> PortFuture<'a, AppCredentialSnapshot> {
        Box::pin(async move {
            self.with_published_registry(|registry| {
                let stored = registry
                    .credentials
                    .iter()
                    .find(|entry| entry.metadata.id == id.as_str())
                    .ok_or(AccessError::InvalidCredential)?;
                scoped_snapshot(registry, stored)
            })
        })
    }
}

fn scoped_snapshot(
    registry: &Registry,
    stored: &StoredCredential,
) -> Result<AppCredentialSnapshot, AccessError> {
    let access = access_snapshot(registry, stored)?;
    let binding = approved_app_binding(registry, stored, &access)?;
    Ok(AppCredentialSnapshot { access, binding })
}

/// Registration is stable across credentials and scoped to this owner and
/// deployment. A length-prefixed tuple keeps the derived ID bounded and
/// separates its fields without imposing syntax on the opaque IDs.
fn app_principal(
    owner: &PrincipalId,
    organization: &OrganizationId,
    audience: &AudienceId,
    app: &ExtensionAppId,
) -> Result<PrincipalId, DomainError> {
    let mut hash = Sha256::new();
    hash.update(b"nessa.app-principal.v1");
    for part in [
        owner.as_str(),
        organization.as_str(),
        audience.as_str(),
        app.as_str(),
    ] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    PrincipalId::new(format!("app:{}", URL_SAFE_NO_PAD.encode(hash.finalize())))
}

/// An existing subject must have retained approvals from this same
/// registration. Its name alone does not make a legacy subject an app.
pub(super) fn check_app_registration(
    registry: &Registry,
    request: &IssueCredentialRequest,
    scope: &StoredAppScope,
) -> Result<(), LocalStoreError> {
    if !registry
        .principals
        .iter()
        .any(|p| p.id == request.principal.id)
    {
        return Ok(());
    }
    let mut previous = registry
        .credentials
        .iter()
        .filter(|stored| stored.metadata.principal_id == request.principal.id)
        .peekable();
    if previous.peek().is_none() {
        return Err(LocalStoreError::Conflict);
    }
    for stored in previous {
        if !matches!(stored.verifier, StoredProof::App(_)) {
            return Err(LocalStoreError::Conflict);
        }
        let snapshot = access_snapshot(registry, stored).map_err(|_| LocalStoreError::Corrupt)?;
        let binding = approved_app_binding(registry, stored, &snapshot)
            .map_err(|_| LocalStoreError::Corrupt)?;
        if binding.app().as_str() != scope.app_id
            || snapshot.credential.organization_id().as_str() != request.membership.organization_id
            || snapshot.credential.audience_id().as_str() != request.audience_id
        {
            return Err(LocalStoreError::Conflict);
        }
    }
    Ok(())
}

pub(super) fn approved_app_binding(
    registry: &Registry,
    stored: &StoredCredential,
    snapshot: &AccessSnapshot,
) -> Result<AppCredentialBinding, AccessError> {
    let StoredProof::App(AppProof::AppBearer { scope, .. }) = &stored.verifier else {
        return Err(AccessError::InvalidCredential);
    };
    let owner = registry
        .memberships
        .iter()
        .find(|member| member.id == registry.owner_membership_id)
        .ok_or(AccessError::Unavailable)?;
    let credential = &snapshot.credential;
    let app = ExtensionAppId::new(&scope.app_id).map_err(|_| AccessError::Unavailable)?;
    let approver = PrincipalId::new(&owner.principal_id).map_err(|_| AccessError::Unavailable)?;
    let request_id = snapshot
        .origin
        .correlation()
        .ok_or(AccessError::Unavailable)?;
    let mut receipts = registry
        .issue_receipts
        .iter()
        .filter(|r| r.credential_id == credential.id().as_str());
    let receipt = receipts.next().ok_or(AccessError::Unavailable)?;
    let principal = registry
        .principals
        .iter()
        .find(|p| p.id == credential.principal_id().as_str())
        .ok_or(AccessError::Unavailable)?;
    let member = registry
        .memberships
        .iter()
        .find(|m| m.id == snapshot.membership.id().as_str())
        .ok_or(AccessError::Unavailable)?;
    let expected_principal = app_principal(
        &approver,
        credential.organization_id(),
        credential.audience_id(),
        &app,
    )
    .map_err(|_| AccessError::Unavailable)?;
    if credential.principal_id() != &expected_principal
        || member.id != expected_principal.as_str()
        || receipts.next().is_some()
        || receipt.issuer_principal_id != approver.as_str()
        || receipt.request_id != request_id
        || owner.organization_id != credential.organization_id().as_str()
        || principal.kind != PrincipalKindDto::Integration
        || member.role != MembershipRoleDto::Member
        || credential.expires_at().is_none()
    {
        return Err(AccessError::IdentityMismatch);
    }
    let approved_request = IssueCredentialRequest {
        purpose: CredentialIssuePurpose::Product,
        request_id: request_id.into(),
        issuer_principal_id: approver.as_str().into(),
        credential_id: credential.id().as_str().into(),
        principal: principal.clone(),
        membership: member.clone(),
        audience_id: credential.audience_id().as_str().into(),
        issued_at: credential.issued_at(),
        expires_at: credential.expires_at(),
        grants: stored.metadata.grants.clone(),
    };
    if issue_fingerprint(&approved_request, Some(scope))
        .ok()
        .as_deref()
        != Some(&receipt.fingerprint)
    {
        return Err(AccessError::IdentityMismatch);
    }
    let approved = ApprovedAppIssue {
        app,
        capabilities: AppCapabilities::restore(&scope.operations)?,
        organization: credential.organization_id().clone(),
        audience: credential.audience_id().clone(),
        subject: credential.principal_id().clone(),
        approving_owner: approver,
        origin: snapshot.origin.clone(),
    };
    AppCredentialBinding::lookup(snapshot, Some(&approved))
}

#[cfg(test)]
mod tests {
    use super::super::{
        tests::{bootstrap, open_store, ready},
        RevokeCredentialRequest,
    };
    use super::*;
    use crate::application::{
        ports::{PortFuture, SessionEvidence, SessionVerifier, VerifiedSessionCredential},
        session::{
            AuthenticateAppSession, AuthenticateSession, ReadCurrentAppSession, ResumeSession,
        },
    };
    use crate::{
        adapters::cedar::CedarPolicyEvaluator,
        application::ports::AccessReader,
        domain::{AudienceId, AuthContext, IssuanceCause},
    };
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Time(u64);
    impl Clock for Time {
        fn unix_milliseconds(&self) -> u64 {
            self.0 * 1000
        }
    }

    struct OpaqueProof(CredentialId);
    impl SessionVerifier for OpaqueProof {
        fn verify_session<'a>(
            &'a self,
            _: &'a SessionEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, VerifiedSessionCredential> {
            Box::pin(async {
                Ok(VerifiedSessionCredential {
                    credential_id: self.0.clone(),
                })
            })
        }
    }

    fn request(id: &str, app: &str, capability: AppCapability) -> IssueAppCredential {
        IssueAppCredential {
            request_id: format!("register-{id}"),
            credential_id: CredentialId::new(id).unwrap(),
            app: ExtensionAppId::new(app).unwrap(),
            capabilities: AppCapabilities::new(capability, &[]),
            expires_at: 180,
        }
    }

    #[test]
    fn app_issue_reopens_and_revokes_without_product_entry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let bootstrap = store.bootstrap(bootstrap()).unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway-1").unwrap();
        let clock = Time(110);
        let auth = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &clock,
        };
        let owner = ready(auth.execute(&bootstrap.evidence, &audience)).unwrap();
        assert!(owner.context().app_namespace().is_default());
        let app_a = CredentialId::new("app-credential-a").unwrap();
        let app_b = CredentialId::new("app-credential-b").unwrap();
        let issued = ready(store.issue_app(
            &owner,
            request(app_a.as_str(), "calendar", AppCapability::Read),
            &clock,
            &policy,
        ))
        .unwrap();
        let IssueCredentialOutcome::Issued {
            metadata,
            evidence,
            transitions,
        } = issued
        else {
            panic!("first approval records a secret once");
        };
        assert_eq!(metadata.issued_at, 110);
        assert!(matches!(
            ready(auth.execute(&evidence, &audience)),
            Err(AccessError::InvalidCredential)
        ));
        let app_snapshot = ready(store.read(&app_a)).unwrap();
        assert_eq!(app_snapshot.origin.cause(), IssuanceCause::AppIssue);
        assert_ne!(app_snapshot.origin.cause(), IssuanceCause::WebhookIssue);
        let [marker] = app_snapshot.credential.grants() else {
            panic!("app identity has one marker, not Product grants");
        };
        assert_eq!(marker.action().as_str(), "app.identity");
        assert!(!store
            .active_device_credential_ids(&audience)
            .unwrap()
            .contains(&app_a));
        let resume = ResumeSession {
            policy: &policy,
            verifier: &OpaqueProof(app_a.clone()),
            access: &store,
            clock: &clock,
        };
        let opaque = SessionEvidence::new(b"synthetic-browser-proof".to_vec()).unwrap();
        assert!(matches!(
            ready(resume.execute(&opaque, &audience)),
            Err(AccessError::Denied)
        ));
        ready(store.issue_app(
            &owner,
            request(app_b.as_str(), "notes", AppCapability::Stop),
            &clock,
            &policy,
        ))
        .unwrap();
        drop(store);

        let store = open_store(&path).unwrap();
        let reopen_time = Time(120);
        let reopen_auth = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &reopen_time,
        };
        let owner = ready(reopen_auth.execute(&bootstrap.evidence, &audience)).unwrap();
        let before_replay = fs::read(&path).unwrap();
        let replay = ready(store.issue_app(
            &owner,
            request(app_a.as_str(), "calendar", AppCapability::Read),
            &reopen_time,
            &policy,
        ))
        .unwrap();
        let IssueCredentialOutcome::ExistingSecretUnavailable {
            metadata: repeated_metadata,
            transitions: repeated_transitions,
        } = replay
        else {
            panic!("the reopened registry returns the existing outcome without another secret");
        };
        assert_eq!(repeated_metadata, metadata);
        assert_eq!(repeated_transitions, transitions);
        for (app, operation) in [
            ("calendar", AppCapability::Stop),
            ("notes", AppCapability::Read),
        ] {
            assert!(matches!(
                ready(store.issue_app(
                    &owner,
                    request(app_a.as_str(), app, operation),
                    &reopen_time,
                    &policy
                )),
                Err(LocalStoreError::Conflict)
            ));
        }
        assert_eq!(fs::read(&path).unwrap(), before_replay);
        let binding = store.app_binding(&app_a, 120).unwrap();
        assert_eq!(binding.app().as_str(), "calendar");
        assert_eq!(
            binding.capabilities().iter().collect::<Vec<_>>(),
            [AppCapability::Read]
        );
        assert_eq!(
            store.app_binding(&app_b, 120).unwrap().app().as_str(),
            "notes"
        );
        assert!(matches!(
            store.app_binding(&app_b, 180),
            Err(AccessError::CredentialExpired)
        ));
        assert!(matches!(
            store.app_binding(owner.context().credential_id(), 120),
            Err(AccessError::InvalidCredential)
        ));
        store
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-calendar".into(),
                issuer_principal_id: owner.context().principal_id().as_str().into(),
                credential_id: app_a.as_str().into(),
                revoked_at: 130,
            })
            .unwrap();
        assert!(matches!(
            store.app_binding(&app_a, 130),
            Err(AccessError::CredentialRevoked)
        ));
        assert!(store.app_binding(&app_b, 130).is_ok());
        let auth = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &Time(130),
        };
        assert!(ready(auth.execute(&bootstrap.evidence, &audience))
            .unwrap()
            .context()
            .app_namespace()
            .is_default());
        drop(store);
        let reopened = open_store(&path).unwrap();
        assert!(matches!(
            reopened.app_binding(&app_a, 130),
            Err(AccessError::CredentialRevoked)
        ));
    }

    #[test]
    fn app_session_uses_only_app_proof_and_current_registry_binding() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(path).unwrap();
        let bootstrap = store.bootstrap(bootstrap()).unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway-1").unwrap();
        let another_gateway = AudienceId::new("other-gateway").unwrap();
        let clock = Time(110);
        let ordinary = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &clock,
        };
        let app_auth = AuthenticateAppSession {
            credentials: &store,
            clock: &clock,
            policy: &policy,
        };
        let owner = ready(ordinary.execute(&bootstrap.evidence, &audience)).unwrap();
        assert!(owner.context().app_namespace().is_default());
        assert!(matches!(
            ready(app_auth.execute(&bootstrap.evidence, &audience)),
            Err(AccessError::InvalidCredential)
        ));

        let mut apps = Vec::new();
        for (id, name, op) in [
            ("app-a", "calendar", AppCapability::List),
            ("app-b", "notes", AppCapability::Read),
        ] {
            let IssueCredentialOutcome::Issued { evidence, .. } =
                ready(store.issue_app(&owner, request(id, name, op), &clock, &policy)).unwrap()
            else {
                panic!("one new app credential")
            };
            assert!(matches!(
                ready(ordinary.execute(&evidence, &audience)),
                Err(AccessError::InvalidCredential)
            ));
            assert!(matches!(
                ready(app_auth.execute(&evidence, &another_gateway)),
                Err(AccessError::InvalidCredential)
            ));
            let raw_id = CredentialEvidence::new(id.as_bytes().to_vec()).unwrap();
            assert!(matches!(
                ready(app_auth.execute(&raw_id, &audience)),
                Err(AccessError::InvalidCredential)
            ));
            let session = ready(app_auth.execute(&evidence, &audience)).unwrap();
            assert_eq!(session.context().app_namespace().as_str(), name);
            assert_eq!(session.context().credential_id().as_str(), id);
            assert_eq!(session.expires_at(), Some(180));
            apps.push(session);
        }
        assert_ne!(
            apps[0].context().principal_id(),
            apps[1].context().principal_id()
        );
        let current = ReadCurrentAppSession {
            credentials: &store,
            clock: &Time(120),
        };
        for (session, expected) in apps.iter().zip([AppCapability::List, AppCapability::Read]) {
            assert_eq!(
                ready(current.execute(session))
                    .unwrap()
                    .capabilities()
                    .iter()
                    .collect::<Vec<_>>(),
                [expected]
            );
        }
        store
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-calendar".into(),
                issuer_principal_id: owner.context().principal_id().as_str().into(),
                credential_id: apps[0].context().credential_id().as_str().into(),
                revoked_at: 120,
            })
            .unwrap();
        assert!(matches!(
            ready(current.execute(&apps[0])),
            Err(AccessError::CredentialRevoked)
        ));
        assert!(ready(current.execute(&apps[1])).is_ok());
        let expired = ReadCurrentAppSession {
            credentials: &store,
            clock: &Time(180),
        };
        assert!(matches!(
            ready(expired.execute(&apps[1])),
            Err(AccessError::CredentialExpired)
        ));
    }

    #[test]
    fn app_registration_id_has_bounded_encoding_and_unambiguous_fields() {
        let audience = AudienceId::new("same-gateway").unwrap();
        let app = ExtensionAppId::new("calendar").unwrap();
        let other_audience = AudienceId::new("another-gateway").unwrap();
        let other_app = ExtensionAppId::new("notes").unwrap();
        let derive = |owner, org, audience, app| {
            app_principal(
                &PrincipalId::new(owner).unwrap(),
                &OrganizationId::new(org).unwrap(),
                audience,
                app,
            )
            .unwrap()
        };
        let first = derive("a", "bc", &audience, &app);
        assert_ne!(first, derive("ab", "c", &audience, &app));
        assert_ne!(first, derive("a", "bc", &other_audience, &app));
        assert_ne!(first, derive("a", "bc", &audience, &other_app));
        assert_eq!(first.as_str().len(), "app:".len() + 43);
    }

    #[test]
    fn app_reissue_keeps_registration_and_each_credential_remains_independent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let bootstrap = store.bootstrap(bootstrap()).unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway-1").unwrap();
        let clock = Time(110);
        let owner = ready(
            AuthenticateSession {
                policy: &policy,
                verifier: &store,
                access: &store,
                clock: &clock,
            }
            .execute(&bootstrap.evidence, &audience),
        )
        .unwrap();

        let mut first_subject = None;
        for (id, operation) in [
            ("app-a", AppCapability::Read),
            ("app-b", AppCapability::Stop),
        ] {
            let IssueCredentialOutcome::Issued {
                metadata,
                transitions,
                ..
            } = ready(store.issue_app(&owner, request(id, "calendar", operation), &clock, &policy))
                .unwrap()
            else {
                panic!("each distinct credential has a fresh issue receipt")
            };
            assert_eq!(
                transitions[0].correlation.as_deref(),
                Some(format!("register-{id}").as_str())
            );
            assert_eq!(metadata.id, id);
            if let Some(first) = &first_subject {
                assert_eq!(&metadata.principal_id, first);
            } else {
                first_subject = Some(metadata.principal_id);
            }
        }
        let mut ambiguous_retry = request("app-c", "calendar", AppCapability::Read);
        ambiguous_retry.request_id = "register-app-a".into();
        assert!(matches!(
            ready(store.issue_app(&owner, ambiguous_retry, &clock, &policy)),
            Err(LocalStoreError::Conflict)
        ));
        store
            .revoke_sync(RevokeCredentialRequest {
                request_id: "revoke-app-a".into(),
                issuer_principal_id: owner.context().principal_id().as_str().into(),
                credential_id: "app-a".into(),
                revoked_at: 120,
            })
            .unwrap();
        drop(store);
        let store = open_store(&path).unwrap();
        let first = CredentialId::new("app-a").unwrap();
        let second = CredentialId::new("app-b").unwrap();
        assert!(matches!(
            store.app_binding(&first, 120),
            Err(AccessError::CredentialRevoked)
        ));
        let preserved = store.app_binding(&second, 120).unwrap();
        assert_eq!(preserved.app().as_str(), "calendar");
        assert_eq!(
            preserved.capabilities().iter().collect::<Vec<_>>(),
            [AppCapability::Stop]
        );
        let current_first = ready(store.read(&first)).unwrap();
        let current_second = ready(store.read(&second)).unwrap();
        assert_eq!(
            current_first.credential.principal_id(),
            current_second.credential.principal_id()
        );
    }

    #[test]
    fn app_issue_does_not_adopt_a_colliding_legacy_principal() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(&path).unwrap();
        let bootstrap = store.bootstrap(bootstrap()).unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway-1").unwrap();
        let clock = Time(110);
        let auth = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &clock,
        };
        let owner = ready(auth.execute(&bootstrap.evidence, &audience)).unwrap();
        let proposal = request("app-new", "calendar", AppCapability::Read);
        let context = owner.context();
        let collision = app_principal(
            context.principal_id(),
            context.organization_id(),
            &audience,
            &proposal.app,
        )
        .unwrap()
        .as_str()
        .to_owned();
        let organization = context.organization_id().as_str();
        let IssueCredentialOutcome::Issued { evidence, .. } = store
            .issue_sync(IssueCredentialRequest {
                purpose: CredentialIssuePurpose::Product,
                request_id: "ordinary-member".into(),
                issuer_principal_id: context.principal_id().as_str().into(),
                credential_id: "legacy-credential".into(),
                principal: PrincipalInputDto {
                    id: collision.clone(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: collision.clone(),
                    principal_id: collision,
                    organization_id: organization.into(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: audience.as_str().into(),
                issued_at: 105,
                expires_at: Some(180),
                grants: vec![CredentialGrantDto {
                    action: "server.read".into(),
                    resource: ResourceDto {
                        organization_id: organization.into(),
                        id: audience.as_str().into(),
                    },
                }],
            })
            .unwrap()
        else {
            panic!("ordinary credential can use the synthetic colliding legacy name")
        };
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            ready(store.issue_app(&owner, proposal, &clock, &policy)),
            Err(LocalStoreError::Conflict)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(ready(auth.execute(&evidence, &audience))
            .unwrap()
            .context()
            .app_namespace()
            .is_default());
    }

    struct BeforeCommit<'a> {
        cedar: &'a CedarPolicyEvaluator,
        advance: &'a (dyn Fn() + Sync),
    }

    impl PolicyEvaluator for BeforeCommit<'_> {
        fn evaluate(
            &self,
            context: &AuthContext,
            action: &Action,
            resource: &Resource,
            snapshot: &AccessSnapshot,
        ) -> Result<Decision, AccessError> {
            let decision = self.cedar.evaluate(context, action, resource, snapshot);
            assert_eq!(decision, Ok(Decision::Allow));
            (self.advance)();
            decision
        }
    }

    struct MovingTime(AtomicU64);
    impl Clock for MovingTime {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::Relaxed) * 1000
        }
    }

    #[test]
    fn app_issue_does_not_commit_if_its_approval_snapshot_or_deadline_changes() {
        for revoke_owner in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("auth/credentials.v1.json");
            let store = open_store(&path).unwrap();
            let bootstrap = store.bootstrap(bootstrap()).unwrap();
            let cedar = CedarPolicyEvaluator::new().unwrap();
            let audience = AudienceId::new("gateway-1").unwrap();
            let clock = MovingTime(AtomicU64::new(110));
            let owner = ready(
                AuthenticateSession {
                    policy: &cedar,
                    verifier: &store,
                    access: &store,
                    clock: &clock,
                }
                .execute(&bootstrap.evidence, &audience),
            )
            .unwrap();
            let before = fs::read(&path).unwrap();
            let advance = || {
                if revoke_owner {
                    store
                        .revoke_sync(RevokeCredentialRequest {
                            request_id: "revoke-owner".into(),
                            issuer_principal_id: owner.context().principal_id().as_str().into(),
                            credential_id: owner.context().credential_id().as_str().into(),
                            revoked_at: 110,
                        })
                        .unwrap();
                } else {
                    clock.0.store(200, Ordering::Relaxed);
                }
            };
            let policy = BeforeCommit {
                cedar: &cedar,
                advance: &advance,
            };
            let mut approval = request("app-late", "notes", AppCapability::Read);
            // It is the approving session, not the proposed app credential,
            // that expires during this second case.
            approval.expires_at = 250;
            assert!(matches!(
                ready(store.issue_app(&owner, approval, &clock, &policy)),
                Err(LocalStoreError::Conflict)
            ));
            assert!(matches!(
                store.app_binding(&CredentialId::new("app-late").unwrap(), 110),
                Err(AccessError::InvalidCredential)
            ));
            if revoke_owner {
                let current = ready(store.read(owner.context().credential_id())).unwrap();
                assert_eq!(current.credential.revoked_at(), Some(110));
                assert_eq!(current.revision, owner.auth_revision() + 1);
            } else {
                assert_eq!(fs::read(&path).unwrap(), before);
            }
        }
    }

    #[test]
    fn app_owner_approval_failure_does_not_publish_a_credential() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth/credentials.v1.json");
        let store = open_store(path).unwrap();
        let bootstrap = store.bootstrap(bootstrap()).unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway-1").unwrap();
        let clock = Time(110);
        let auth = AuthenticateSession {
            policy: &policy,
            verifier: &store,
            access: &store,
            clock: &clock,
        };
        let owner = ready(auth.execute(&bootstrap.evidence, &audience)).unwrap();
        let nonowner = store
            .provision_surface(
                "separate-surface",
                "provision".into(),
                "nonowner".into(),
                vec!["credential.manage".into()],
                105,
                Some(200),
            )
            .unwrap();
        let IssueCredentialOutcome::Issued { evidence, .. } = nonowner else {
            panic!("new test surface")
        };
        let other = ready(auth.execute(&evidence, &audience)).unwrap();
        let before = std::fs::read(store.root.join(&store.path)).unwrap();
        assert!(matches!(
            ready(store.issue_app(
                &other,
                request("app-new", "notes", AppCapability::Read),
                &clock,
                &policy
            )),
            Err(LocalStoreError::Conflict)
        ));
        store.fail_before_replace.store(true, Ordering::Release);
        assert!(matches!(
            ready(store.issue_app(
                &owner,
                request("app-new", "notes", AppCapability::Read),
                &clock,
                &policy
            )),
            Err(LocalStoreError::Io(_))
        ));
        assert_eq!(std::fs::read(store.root.join(&store.path)).unwrap(), before);
        assert!(matches!(
            store.app_binding(&CredentialId::new("app-new").unwrap(), 110),
            Err(AccessError::InvalidCredential)
        ));
    }
}
