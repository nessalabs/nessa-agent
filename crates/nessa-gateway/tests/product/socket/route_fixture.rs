// The product route state the socket tests and composition's host-cleanup
// tests build on: a fixed credential authority, its access snapshots, and
// the passive-read substitutes.
use super::*;
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::{
    ConversationRepository, ReceiverAuthority, ReceiverBinding, RecordReadError,
    RecordReadFuture, RecordReadOperation, RecordReadResponse, RecordReadSource,
    ReviewNoticeAuditor,
};
use crate::conversation::domain::Conversation;
use crate::conversation::infrastructure::{
    LocalConversationStore, MonotonicReviewNoticeClock,
};
use crate::product::ProductDependencies;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::ports::{
    AccessReader, AccessSnapshot, Clock, CredentialVerifier, PortFuture,
    VerifiedCredential,
};
use nessa_auth::domain::{
    AudienceId, Credential, CredentialId, Grant, Membership, MembershipId,
    MembershipRole, MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::clock::Clock as UptimeClock;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::read_scope::{ReadRefusal, ReceiverReadScope};
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::application::agent_execution::sessions::{
    ProviderContext, SessionChange, SessionSaveUnit, SessionStorage,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct RecordBinding;

impl ReceiverAuthority for RecordBinding {
    fn resolve<'a>(
        &'a self,
        _: &'a CredentialId,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ReceiverBinding>, ReadRefusal>> + Send + 'a>>
    {
        Box::pin(async {
            Ok(Some(ReceiverBinding {
                receiver_id: "receiver".into(),
                credential_id: CredentialId::new("credential").unwrap(),
                organization_id: OrganizationId::new("organization").unwrap(),
                owner_id: PrincipalId::new("principal").unwrap(),
                access_epoch: 3,
                active: true,
            }))
        })
    }
}

pub struct RecordAdmissionSpy(pub AtomicU64);

impl RecordReadSource for RecordAdmissionSpy {
    fn read<'a>(
        &'a self,
        _: ReceiverReadScope,
        _: RecordReadOperation,
        _: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(RecordReadError::TemporarilyUnavailable) })
    }
}

pub struct Authority {
    pub snapshot: Mutex<AccessSnapshot>,
    pub now: AtomicU64,
    pub proof_expires_at: Mutex<Option<u64>>,
    /// A share target's principal, by credential id, when it is not the
    /// session's. Absent, every read is the session's actor.
    pub target_principal: Mutex<std::collections::HashMap<String, PrincipalId>>,
}

impl CredentialVerifier for Authority {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            if evidence.expose_bytes() != b"secret" || audience.as_str() != "gateway" {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id: CredentialId::new("credential").unwrap(),
                expires_at: *self.proof_expires_at.lock().unwrap(),
            })
        })
    }
}

impl AccessReader for Authority {
    /// The one configured snapshot, answered about the credential asked
    /// for: a share target reads as the session's own actor.
    fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            let named = &snapshot.credential;
            let principal = self
                .target_principal
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(credential.as_str())
                .cloned()
                .unwrap_or_else(|| named.principal_id().clone());
            if named.id() != credential || principal != *named.principal_id() {
                snapshot.credential = Credential::new(
                    credential.clone(),
                    principal.clone(),
                    named.organization_id().clone(),
                    named.audience_id().clone(),
                    named.issued_at(),
                    named.expires_at(),
                    named.grants().to_vec(),
                )
                .unwrap();
                let membership = snapshot.membership.clone();
                snapshot.membership = Membership::new(
                    membership.id().clone(),
                    principal,
                    membership.organization_id().clone(),
                    membership.role(),
                    membership.status(),
                );
            }
            Ok(snapshot)
        })
    }
}

impl Clock for Authority {
    fn unix_milliseconds(&self) -> u64 {
        self.now.load(Ordering::SeqCst) * 1000
    }
}

impl UptimeClock for Authority {
    fn elapsed_ms(&self) -> u64 {
        42
    }
}

/// Original issuance of a newly constructed synthetic product credential.
pub fn test_product_origin(
    credential: &Credential,
) -> nessa_auth::application::origin::CredentialOrigin {
    let original = credential
        .issued(
            nessa_auth::domain::IssuanceCause::AdminIssue,
            nessa_auth::domain::Initiator::LocalOperator,
        )
        .unwrap();
    nessa_auth::application::origin::CredentialOrigin::restore(
        nessa_auth::application::dto::CredentialTransitionDto::record(&original, 1, 1, None),
        credential,
    )
    .unwrap()
}

pub fn snapshot(role: MembershipRole, status: MembershipStatus) -> AccessSnapshot {
    snapshot_with(role, status, &["server.read", "credential.manage"])
}

pub fn snapshot_with(
    role: MembershipRole,
    status: MembershipStatus,
    actions: &[&str],
) -> AccessSnapshot {
    let organization = OrganizationId::new("organization").unwrap();
    let resource = Resource::new(
        organization.clone(),
        ResourceId::new("gateway-resource").unwrap(),
    );
    {
        let credential = Credential::new(
            CredentialId::new("credential").unwrap(),
            PrincipalId::new("principal").unwrap(),
            organization.clone(),
            AudienceId::new("gateway").unwrap(),
            100,
            200,
            actions
                .iter()
                .map(|action| Grant::new(Action::new(*action).unwrap(), resource.clone()))
                .collect(),
        )
        .unwrap();
        let membership = Membership::new(
            MembershipId::new("membership").unwrap(),
            PrincipalId::new("principal").unwrap(),
            organization,
            role,
            status,
        );
        let revision = 1;
        let origin = test_product_origin(&credential);
        AccessSnapshot {
            revocation: None,
            credential,
            membership,
            revision,
            origin,
        }
    }
}

impl nessa_auth::application::pairing::OwnerPhonePurposeSource for Authority {
    fn owner_phone_purpose<'a>(
        &'a self,
        _: &'a AuthenticatedSession,
        _: &'a dyn nessa_auth::application::ports::Clock,
    ) -> nessa_auth::application::ports::PortFuture<
        'a,
        Option<nessa_auth::application::pairing::OwnerPhonePurpose>,
    > {
        // This substitute models ordinary credentials without a pairing ledger.
        Box::pin(async { Ok(None) })
    }
}

pub fn fixture(role: MembershipRole) -> (ProductRouteState, Arc<Authority>) {
    let authority = Arc::new(Authority {
        snapshot: Mutex::new(snapshot(role, MembershipStatus::Active)),
        now: AtomicU64::new(100),
        proof_expires_at: Mutex::new(Some(200)),
        target_principal: Mutex::new(std::collections::HashMap::new()),
    });
    let state = ProductRouteState::new(
        ResourceId::new("gateway-resource").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: authority.clone(),
            owner_phone_purpose: authority.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new()
                    .unwrap(),
            ),
            owner_phone_receivers: None,
            access: authority.clone(),
            clock: authority.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: authority.clone(),
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    )
    .with_review_notice_auditor(Arc::new(ReviewNoticeAuditor::new(
        Arc::new(conversation_support::RecordingReviewNoticeAudit::default()),
        Arc::new(MonotonicReviewNoticeClock::new()),
    )));
    (state, authority)
}
