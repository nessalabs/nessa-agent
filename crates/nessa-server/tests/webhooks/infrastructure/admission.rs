use crate::conversation::{
    application::{
        AccessAction, AccessAsk, AccessDecision, AccessResource, AccessSubject,
        ConversationAccessPolicyEvaluator, ConversationCaller, ConversationRepository,
        RequestShape, ShareGrantChange, ShareGrantTransition, ShareGrants, ShareTarget,
    },
    domain::{Conversation, ShareAccess},
    infrastructure::{CedarConversationAccessPolicy, LocalConversationStore},
};
use crate::webhooks::application::{AdmissionFailure, AdmitDelivery, Target};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
    },
    application::{
        credential_admin::{
            CredentialIssuePurpose, IssueCredentialRequest, RevokeCredentialRequest,
        },
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::{AccessReader, Clock},
    },
    domain::{CredentialId, OrganizationId, PrincipalId, Resource, ResourceId},
};
use nessa_protocol::{
    agents::AgentId,
    conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId},
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
use uuid::Uuid;

struct Time(AtomicU64);
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    credentials: LocalCredentialStore,
    grants: LocalConversationStore,
    policy: CedarConversationAccessPolicy,
    gateway: Resource,
    clock: Time,
    target: Target,
    a: ConversationId,
    b: ConversationId,
    epoch: u64,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_purpose(CredentialIssuePurpose::Product).await
    }
    async fn with_purpose(purpose: CredentialIssuePurpose) -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let credentials = LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let org = OrganizationId::new("org").unwrap();
        let owner = PrincipalId::new("owner").unwrap();
        let gateway = Resource::new(org.clone(), ResourceId::new("gateway").unwrap());
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        };
        credentials
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "owner-key".into(),
                issued_at: 1,
                expires_at: None,
                grants: vec![grant("credential.manage")],
            })
            .unwrap();
        credentials
            .issue_sync(IssueCredentialRequest {
                purpose,
                request_id: "issue-synthetic-hook".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "hook-key".into(),
                principal: PrincipalInputDto {
                    id: "hook".into(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: "hook-member".into(),
                    principal_id: "hook".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: "gateway".into(),
                issued_at: 1,
                expires_at: Some(100),
                grants: vec![grant("conversation.read")],
            })
            .unwrap();
        let policy = CedarConversationAccessPolicy::new().unwrap();
        let grants = LocalConversationStore::open(&private.join("conversations.db"))
            .unwrap()
            .with_access_policy(Arc::new(CedarConversationAccessPolicy::new().unwrap()));
        let a = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        let b = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        for id in [&a, &b] {
            let _created = grants
                .create(
                    Conversation::new(
                        id.clone(),
                        org.clone(),
                        owner.clone(),
                        "test".into(),
                        format!("create-{id}"),
                        1,
                        AgentId::Claude,
                        ConversationModelId::new("synthetic").unwrap(),
                        ConversationApprovalMode::Ask,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let target = Target {
            credential: CredentialId::new("hook-key").unwrap(),
            principal: PrincipalId::new("hook").unwrap(),
            organization: org,
            owner,
        };
        let mut fixture = Self {
            _root: root,
            credentials,
            grants,
            policy,
            gateway,
            clock: Time(AtomicU64::new(2000)),
            target,
            a,
            b,
            epoch: 0,
        };
        fixture.epoch = fixture
            .change(ShareGrantTransition::Grant(ShareAccess::read()), "grant")
            .await
            .unwrap();
        fixture
    }
    fn admission(&self) -> AdmitDelivery<'_> {
        AdmitDelivery {
            credentials: &self.credentials,
            grants: &self.grants,
            conversations: &self.grants,
            policy: &self.policy,
            clock: &self.clock,
            gateway: &self.gateway,
        }
    }
    async fn change(&self, transition: ShareGrantTransition, request: &str) -> Option<u64> {
        self.grants
            .change(ShareGrantChange {
                transition,
                conversation_id: self.a.clone(),
                principal_id: Some(self.target.principal.clone()),
                receiver_id: None,
                credential_id: self.target.credential.clone(),
                expected_policy_revision: None,
                initiator: ConversationCaller {
                    organization_id: self.target.organization.clone(),
                    principal_id: self.target.owner.clone(),
                    surface_id: "synthetic".into(),
                    action_id: request.into(),
                },
                at_ms: self.clock.unix_milliseconds(),
            })
            .await
            .unwrap()
            .policy_revision
    }
}

#[tokio::test]
async fn principal_admission_filters_chats_revoke_epochs_and_lifetime() {
    let f = Fixture::new().await;
    let hold = f
        .admission()
        .execute(&f.target, &f.a, f.epoch)
        .await
        .unwrap();
    assert!(hold.is_valid_at(99));
    assert!(!hold.is_valid_at(100));
    drop(hold);
    assert!(matches!(
        f.admission().execute(&f.target, &f.b, f.epoch).await,
        Err(AdmissionFailure::Refused)
    ));
    f.change(ShareGrantTransition::Revoke, "unshare").await;
    assert!(matches!(
        f.admission().execute(&f.target, &f.a, f.epoch).await,
        Err(AdmissionFailure::Refused)
    ));
    let next = f
        .change(ShareGrantTransition::Grant(ShareAccess::read()), "reshare")
        .await
        .unwrap();
    assert!(next > f.epoch);
    assert!(matches!(
        f.admission().execute(&f.target, &f.a, f.epoch).await,
        Err(AdmissionFailure::Refused)
    ));
    drop(f.admission().execute(&f.target, &f.a, next).await.unwrap());
    f.credentials
        .revoke_sync(RevokeCredentialRequest {
            request_id: "revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "hook-key".into(),
            revoked_at: 3,
        })
        .unwrap();
    assert!(matches!(
        f.admission().execute(&f.target, &f.a, next).await,
        Err(AdmissionFailure::Refused)
    ));
}

#[tokio::test]
async fn credential_expiring_while_waiting_for_grant_is_refused() {
    let f = Fixture::new().await;
    let grant_hold = f
        .grants
        .hold_principal_revision(&f.a, &f.target.principal, f.epoch)
        .await
        .unwrap();
    let admission = f.admission();
    let mut waiting = std::pin::pin!(admission.execute(&f.target, &f.a, f.epoch));
    assert!(matches!(
        waiting
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    ));
    f.clock.0.store(100_000, Ordering::SeqCst);
    drop(grant_hold);
    assert!(matches!(waiting.await, Err(AdmissionFailure::Refused)));
}

#[tokio::test]
async fn webhook_cedar_subject_has_no_send_or_owner_authority() {
    let f = Fixture::new().await;
    let metadata = f.grants.load(&f.a).await.unwrap().unwrap();
    let snapshot = f.credentials.read(&f.target.credential).await.unwrap();
    for action in AccessAction::ALL {
        let resource = match action {
            AccessAction::List | AccessAction::Write => AccessResource::Catalogue {
                organization: &f.target.organization,
                owner: &f.target.owner,
            },
            AccessAction::WorkspaceRead => AccessResource::Workspace {
                organization: &f.target.organization,
                owner: &f.target.owner,
            },
            _ => AccessResource::of(&metadata),
        };
        let verdict = f.policy.evaluate(&AccessAsk {
            subject: AccessSubject::WebhookReader {
                organization: &f.target.organization,
                principal: &f.target.principal,
                registration_owner: &f.target.owner,
            },
            action,
            resource,
            held: Some(ShareAccess::read().capabilities()),
            request: RequestShape::default(),
            target: Some(ShareTarget {
                organization: &f.target.organization,
                binding_owner: &f.target.owner,
                grants: snapshot.credential.grants(),
                gateway: &f.gateway,
            }),
        });
        assert_eq!(
            matches!(verdict, AccessDecision::Allow { .. }),
            action == AccessAction::Read,
            "{action:?}"
        );
    }
    let other = Target {
        owner: PrincipalId::new("other-owner").unwrap(),
        ..f.target.clone()
    };
    assert!(matches!(
        f.admission().execute(&other, &f.a, f.epoch).await,
        Err(AdmissionFailure::Refused)
    ));
}

#[path = "source.rs"]
mod source_tests;

#[path = "share.rs"]
mod share_tests;
