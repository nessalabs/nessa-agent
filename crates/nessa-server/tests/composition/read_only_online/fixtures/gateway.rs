//! Independent client/gateway processes use canonical credential, receiver and cache owners.
use super::{private_write, uuid, Setup};
use crate::agents::application::{AgentProbe, AgentProbeEvidence};
use crate::agents::domain::AgentId;
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::conversation::application::{
    ConversationRepository, ReceiverReadScope, RecordReadFuture, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadSource,
};
use crate::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationId, ConversationModelId,
};
use crate::conversation::infrastructure::{
    LocalConversationStore, LocalReceiverAuthority, NessaCatalogueReadSource, NessaRecordReadSource,
};
use crate::product::{ProductDependencies, ProductRouteState};
use axum::Extension;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::{BootstrapRequest, LocalCredentialStore};
use nessa_auth::application::credential_admin::{IssueCredentialOutcome, IssueCredentialRequest};
use nessa_auth::application::dto::{
    CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
    OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
};
use nessa_auth::application::ports::{AccessReader, Clock, CredentialVerifier};
use nessa_auth::domain::{AudienceId, OrganizationId, PrincipalId, ResourceId};
use nessa_gateway_endpoint::application::PublishGatewayEndpoint;
use nessa_gateway_endpoint::domain::{
    EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement,
};
use nessa_gateway_endpoint::infrastructure::FileEndpointPublication;
use nessa_local_storage::OpenMode;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::application::agent_execution::sessions::{
    SessionChange, SessionSaveGeneration, SessionSnapshot, SessionStorage,
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use nessa_sync::replication::domain::Id;
use serde_json::json;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::runtime::Handle;

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}
// Deterministic timing at the actual source port; replies still come from SDK storage.
struct GatedRead {
    source: Arc<NessaRecordReadSource>,
    storage: Arc<RecordStorage>,
    conversation: SessionId,
    authority: Arc<LocalReceiverAuthority>,
    receiver: String,
    initiator: PrincipalId,
    mode: String,
    pages: AtomicUsize,
    heads: AtomicUsize,
}
impl RecordReadSource for GatedRead {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        let page = matches!(&operation, RecordReadOperation::Page(_));
        Box::pin(async move {
            let head = (!page).then(|| self.heads.fetch_add(1, Ordering::SeqCst) + 1);
            // Source latency exercises the real socket phase owner (B1), not a
            // fabricated timeout reply. Successful reads still use SDK storage.
            match (self.mode.as_str(), head) {
                ("delayed-head", Some(1)) => tokio::time::sleep(Duration::from_secs(6)).await,
                ("timeout-head", Some(1)) => std::future::pending::<()>().await,
                ("cumulative-head", Some(count)) if count > 1 => {
                    tokio::time::sleep(Duration::from_secs(3)).await
                }
                ("cumulative-head", None) => tokio::time::sleep(Duration::from_secs(7)).await,
                _ => {}
            }
            let response = self.source.read(admitted, operation, lease).await?;
            if !page && self.mode == "before-page" && head == Some(3) {
                self.authority
                    .change(
                        self.receiver.clone(),
                        None,
                        false,
                        self.initiator.clone(),
                        uuid(),
                    )
                    .await
                    .unwrap();
            }
            if page {
                let count = self.pages.fetch_add(1, Ordering::SeqCst) + 1;
                if self.mode == "epoch" && count == 1 {
                    self.authority
                        .change(
                            self.receiver.clone(),
                            None,
                            false,
                            self.initiator.clone(),
                            uuid(),
                        )
                        .await
                        .unwrap();
                }
                if self.mode == "append" && count == 1 {
                    let lease = self.storage.open(self.conversation.clone()).await.unwrap();
                    let mut snapshot = lease.load().await.unwrap().unwrap();
                    let before = snapshot.provider_context.clone();
                    snapshot.provider_context = ProviderContext::Recorded(
                        ExecutionSessionId::new("later-source-context").unwrap(),
                    );
                    let after = snapshot.provider_context.clone();
                    lease
                        .save_changes(
                            SessionSaveGeneration::initial(),
                            snapshot,
                            vec![SessionChange::ProviderContext { before, after }],
                        )
                        .await
                        .unwrap();
                    drop(lease);
                }
                if self.mode == "disconnect" && count == 2 {
                    std::process::exit(0);
                }
            }
            Ok(response)
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_child() {
    let Ok(root) = std::env::var("NESSA_ONLINE_GATEWAY") else {
        return;
    };
    let root = Path::new(&root);
    nessa_local_storage::create_directory(root).unwrap();
    let auth = Arc::new(LocalCredentialStore::open(root, "credentials.json").unwrap());
    let receivers = Arc::new(
        LocalReceiverAuthority::open(
            &root.join("receivers.sqlite3"),
            &CedarPolicyEvaluator::profile_digest(),
            Arc::new(SystemClock),
        )
        .unwrap(),
    );
    let setup_path = root.join("setup.json");
    let setup = if setup_path.exists() {
        serde_json::from_slice::<Setup>(&std::fs::read(&setup_path).unwrap()).unwrap()
    } else {
        let gateway = uuid();
        let organization = uuid();
        let owner = uuid();
        let reader = uuid();
        let credential = uuid();
        let membership = uuid();
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: organization.clone(),
                id: gateway.clone(),
            },
        };
        let boot = auth
            .bootstrap(BootstrapRequest {
                gateway_id: gateway.clone(),
                organization: OrganizationInputDto {
                    id: organization.clone(),
                },
                principal: PrincipalInputDto {
                    id: owner.clone(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: uuid(),
                    principal_id: owner.clone(),
                    organization_id: organization.clone(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: uuid(),
                issued_at: SystemClock.unix_seconds(),
                expires_at: None,
                grants: vec![
                    grant("server.read"),
                    grant("conversation.read"),
                    grant("conversation.write"),
                    grant("credential.manage"),
                ],
            })
            .unwrap();
        auth.verify(&boot.evidence, &AudienceId::new(gateway.clone()).unwrap())
            .await
            .unwrap();
        let issued = auth
            .issue_sync(IssueCredentialRequest {
                request_id: uuid(),
                issuer_principal_id: owner.clone(),
                credential_id: credential.clone(),
                principal: PrincipalInputDto {
                    id: reader.clone(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: membership.clone(),
                    principal_id: reader.clone(),
                    organization_id: organization.clone(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: gateway.clone(),
                issued_at: SystemClock.unix_seconds(),
                expires_at: None,
                grants: vec![grant("conversation.read")],
            })
            .unwrap();
        let IssueCredentialOutcome::Issued {
            metadata, evidence, ..
        } = issued
        else {
            panic!("fresh credential")
        };
        let verified = auth
            .verify(&evidence, &AudienceId::new(gateway.clone()).unwrap())
            .await
            .unwrap();
        assert_eq!(verified.credential_id.as_str(), metadata.id);
        let access = auth.read(&verified.credential_id).await.unwrap();
        assert_eq!(access.credential.principal_id().as_str(), reader);
        let paired = receivers
            .pair(
                verified.credential_id,
                OrganizationId::new(organization.clone()).unwrap(),
                PrincipalId::new(reader.clone()).unwrap(),
                PrincipalId::new(owner.clone()).unwrap(),
                uuid(),
            )
            .await
            .unwrap();
        private_write(&root.join("reader.token"), evidence.expose_bytes());
        let setup = Setup {
            gateway,
            organization,
            owner,
            reader,
            credential,
            membership,
            receiver: paired.receiver_id.clone(),
            epoch: paired.access_epoch,
            conversation: uuid(),
            empty: uuid(),
        };
        private_write(&setup_path, &serde_json::to_vec(&setup).unwrap());
        private_write(&root.join("profile.json"),&serde_json::to_vec(&json!({"receiver":setup.receiver,"accessEpoch":setup.epoch,"credentialFile":root.join("reader.token"),"endpointRoot":root,"endpointDirectory":"endpoint"})).unwrap());
        setup
    };
    if std::env::var("NESSA_ONLINE_GATE").as_deref() == Ok("regrant") {
        receivers
            .change(
                setup.receiver.clone(),
                None,
                false,
                PrincipalId::new(setup.owner.clone()).unwrap(),
                uuid(),
            )
            .await
            .unwrap();
        let issued = auth
            .issue_sync(IssueCredentialRequest {
                request_id: uuid(),
                issuer_principal_id: setup.owner.clone(),
                credential_id: uuid(),
                principal: PrincipalInputDto {
                    id: setup.reader.clone(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: setup.membership.clone(),
                    principal_id: setup.reader.clone(),
                    organization_id: setup.organization.clone(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: setup.gateway.clone(),
                issued_at: SystemClock.unix_seconds(),
                expires_at: None,
                grants: vec![CredentialGrantDto {
                    action: "conversation.read".into(),
                    resource: ResourceDto {
                        organization_id: setup.organization.clone(),
                        id: setup.gateway.clone(),
                    },
                }],
            })
            .unwrap();
        let IssueCredentialOutcome::Issued { evidence, .. } = issued else {
            panic!("explicit fresh regrant credential")
        };
        let verified = auth
            .verify(&evidence, &AudienceId::new(setup.gateway.clone()).unwrap())
            .await
            .unwrap();
        let mut token =
            nessa_local_storage::open(&root.join("reader.token"), OpenMode::ReadWrite).unwrap();
        token.set_len(0).unwrap();
        token.write_all(evidence.expose_bytes()).unwrap();
        token.sync_all().unwrap();
        let binding = receivers
            .change(
                setup.receiver.clone(),
                Some(verified.credential_id),
                true,
                PrincipalId::new(setup.owner.clone()).unwrap(),
                uuid(),
            )
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&json!({"receiver":setup.receiver,"accessEpoch":binding.access_epoch,"credentialFile":root.join("reader.token"),"endpointRoot":root,"endpointDirectory":"endpoint"})).unwrap();
        let mut file =
            nessa_local_storage::open(&root.join("profile.json"), OpenMode::ReadWrite).unwrap();
        file.set_len(0).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
    }
    let metadata = Arc::new(LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap());
    let storage = Arc::new(RecordStorage::new(root.join("source")).unwrap());
    storage.initialize().await.unwrap();
    for (target, large) in [(&setup.conversation, true), (&setup.empty, false)] {
        let id = ConversationId::new(target).unwrap();
        if metadata.load(&id).await.unwrap().is_none() {
            metadata
                .create(
                    Conversation::new(
                        id,
                        OrganizationId::new(setup.organization.clone()).unwrap(),
                        PrincipalId::new(setup.reader.clone()).unwrap(),
                        "setup".into(),
                        uuid(),
                        SystemClock.unix_milliseconds(),
                        AgentId::Claude,
                        ConversationModelId::new("model").unwrap(),
                        ConversationApprovalMode::Ask,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            let session = SessionId::new(target).unwrap();
            let lease = storage.open(session.clone()).await.unwrap();
            if large {
                let provider = ProviderIdentity::new("provider", "model", "workspace").unwrap();
                let mut context = ProviderContext::Absent;
                let mut changes = vec![SessionChange::Opened {
                    id: session.clone(),
                    provider: provider.clone(),
                    context: context.clone(),
                }];
                for index in 0..300 {
                    let next = ProviderContext::Recorded(
                        ExecutionSessionId::new(format!("{index:05}{}", "x".repeat(240))).unwrap(),
                    );
                    changes.push(SessionChange::ProviderContext {
                        before: context,
                        after: next.clone(),
                    });
                    context = next;
                }
                lease
                    .save_changes(
                        SessionSaveGeneration::initial(),
                        SessionSnapshot {
                            id: session,
                            provider,
                            provider_context: context,
                            invocations: vec![],
                            queue_history: vec![],
                        },
                        changes,
                    )
                    .await
                    .unwrap();
            }
            drop(lease);
        }
    }
    let record = Arc::new(NessaRecordReadSource::new(
        storage.clone(),
        Id::new(&setup.gateway).unwrap(),
        Handle::current(),
    ));
    let state = ProductRouteState::new(
        ResourceId::new(setup.gateway.clone()).unwrap(),
        OrganizationId::new(setup.organization.clone()).unwrap(),
        AudienceId::new(setup.gateway.clone()).unwrap(),
        ProductDependencies {
            verifier: auth.clone(),
            access: auth.clone(),
            clock: Arc::new(SystemClock),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: RuntimeDependencies::default().clock,
            agent_probe: Arc::new(NoAgents),
        },
    )
    .with_passive_read(receivers.clone(), metadata.clone())
    .with_record_source(Arc::new(GatedRead {
        source: record.clone(),
        storage: storage.clone(),
        conversation: SessionId::new(setup.conversation.clone()).unwrap(),
        authority: receivers,
        receiver: setup.receiver.clone(),
        initiator: PrincipalId::new(setup.owner.clone()).unwrap(),
        mode: std::env::var("NESSA_ONLINE_GATE").unwrap_or_default(),
        pages: AtomicUsize::new(0),
        heads: AtomicUsize::new(0),
    }))
    .with_catalogue_source(Arc::new(NessaCatalogueReadSource::new(
        metadata,
        Id::new(&setup.gateway).unwrap(),
    )));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        EndpointIdentity::new(uuid(), std::process::id()).unwrap(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&FileEndpointPublication::new(
        root.into(),
        "endpoint".into(),
    ))
    .execute(&GatewayEndpointAdvertisement::new(endpoint.clone(), None).unwrap())
    .unwrap();
    println!("ONLINE_READY");
    io::stdout().flush().unwrap();
    axum::serve(
        listener,
        crate::server::entrypoint::http::router(state)
            .layer(Extension(endpoint.identity().clone())),
    )
    .await
    .unwrap();
    record.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}
