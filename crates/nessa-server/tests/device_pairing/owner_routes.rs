//! Owner pairing methods over the real `/session` route, registry, Cedar and
//! native enrollment runtime. Design rows O1–O4, W1 and S1 in
//! `docs/design/auth/device-pairing.md` ("Owner routes and mounting").
use super::product_client::ProductClient;
use super::support::{pending, Fixture, Time, WAIT};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        pairing::{ManualCode, OsEntropy},
    },
    application::{
        credential_admin::{IssueCredentialOutcome, IssueCredentialRequest},
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        pairing::PairingStore,
    },
    domain::{
        pairing::{InvitationId, PairingPhase},
        AudienceId, OrganizationId, ResourceId,
    },
};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_protocol::agents::AgentId;
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_server::{
    agents::application::{AgentProbe, AgentProbeEvidence},
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{InvitationEntropy, PairingOwnerCommands},
    product::{ProductDependencies, ProductRouteState},
    server::entrypoint::http,
};
use serde_json::{json, Value};
use std::{
    net::{SocketAddr, TcpStream},
    sync::{atomic::Ordering, Arc},
};

struct NoAgents;
impl AgentProbe for NoAgents {
    fn evidence(&self, _: AgentId) -> Option<AgentProbeEvidence> {
        None
    }
}

/// Serve the real product route for `fixture`'s gateway, with or without its
/// owner pairing commands.
async fn serve(fixture: &Fixture, pairing: bool) -> SocketAddr {
    let mut state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: fixture.registry.clone(),
            access: fixture.registry.clone(),
            clock: Arc::new(Time),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: RuntimeDependencies::default().clock,
            agent_probe: Arc::new(NoAgents),
        },
    );
    if pairing {
        state = state.with_pairing(Arc::new(PairingOwnerCommands::new(
            fixture.gateway.clone(),
            Arc::new(|| Box::new(OsEntropy) as Box<dyn InvitationEntropy>),
        )));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, http::router(state)).await });
    address
}

/// Run blocking client work without stalling the runtime serving it.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

fn invitation(status: &Value) -> Value {
    status["invitationId"].clone()
}

/// Row O1: the code is answered only from the committed record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_create_orders_code_after_commit() {
    let fixture = Fixture::new().await;
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    let created =
        blocking(|| ProductClient::connect(address, &token).ok("pairing.create", json!({})));
    let code = created["code"].as_str().unwrap();
    assert!(ManualCode::parse(code.as_bytes()).is_ok(), "{code}");
    let status = &created["status"];
    assert_eq!(status["phase"], "available");
    assert_eq!(status["cleanupPending"], false);
    assert!(status.get("claimedDeviceKey").is_none());
    assert!(status.get("terminal").is_none());
    let id: [u8; 16] = serde_json::from_value(invitation(status)).unwrap();
    let record = fixture
        .registry
        .read_pairing(InvitationId::new(id))
        .unwrap();
    assert_eq!(record.phase(), PairingPhase::Available);
    assert_eq!(status["expiresAtMs"], record.expires_at_ms());
    assert_eq!(status["createdAtMs"], record.created_at_ms());
    assert_eq!(status["generation"], record.intent().generation());
    assert_eq!(
        status["consentId"],
        json!(record.intent().id().bytes().to_vec())
    );
    assert_eq!(
        status["grant"],
        json!({"action": "conversation.read", "resource": {"organizationId": "org", "id": "gateway"}})
    );
}

/// Row O2: a lost create answer keeps its slot; pending finds it without its
/// code, cancel ends it as the owner's, and a new create then succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_lost_create_answer_keeps_slot() {
    let fixture = Fixture::new().await;
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    blocking(|| {
        let mut owner = ProductClient::connect(address, &token);
        let first = owner.ok("pairing.create", json!({}));
        assert_eq!(
            owner.refused("pairing.create", json!({})),
            "pairing_slot_occupied"
        );
        let pending = owner.ok("pairing.pending", json!({}));
        let items = pending["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["invitationId"], invitation(&first["status"]));
        assert!(
            !pending
                .to_string()
                .contains(first["code"].as_str().unwrap()),
            "pending never carries a code"
        );
        let cancelled = owner.ok(
            "pairing.cancel",
            json!({"invitationId": invitation(&first["status"])}),
        );
        assert_eq!(cancelled["phase"], "terminal");
        assert_eq!(cancelled["terminal"]["cause"], "cancelled");
        assert_eq!(
            cancelled["terminal"]["initiator"],
            json!({"kind": "principal", "principalId": "owner"})
        );
        let second = owner.ok("pairing.create", json!({}));
        assert_ne!(invitation(&second["status"]), invitation(&first["status"]));
        // An ended invitation is not pending; the new one is.
        let pending = owner.ok("pairing.pending", json!({}));
        assert_eq!(
            pending["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(invitation)
                .collect::<Vec<_>>(),
            vec![invitation(&second["status"])]
        );
        // Status of the cancelled one still reads its first cause.
        let status = owner.ok(
            "pairing.status",
            json!({"invitationId": invitation(&first["status"])}),
        );
        assert_eq!(status["terminal"]["cause"], "cancelled");
    });
}

/// Row O3: a member session is refused at socket admission, before any
/// registry read, and creates nothing; an unknown invitation is not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_refuses_a_member_before_disclosure() {
    let fixture = Fixture::new().await;
    let IssueCredentialOutcome::Issued { evidence, .. } = fixture
        .registry
        .issue_sync(IssueCredentialRequest {
            request_id: "member-request".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "member-credential".into(),
            principal: PrincipalInputDto {
                id: "member-principal".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member-membership".into(),
                principal_id: "member-principal".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Member,
                state: MembershipStateDto::Active,
            },
            audience_id: "gateway".into(),
            issued_at: 100,
            expires_at: None,
            grants: vec![CredentialGrantDto {
                action: "conversation.read".into(),
                resource: ResourceDto {
                    organization_id: "org".into(),
                    id: "gateway".into(),
                },
            }],
        })
        .unwrap()
    else {
        panic!("a new member credential is issued");
    };
    let member = String::from_utf8(evidence.expose_bytes().to_vec()).unwrap();
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    let owned =
        blocking(|| ProductClient::connect(address, &token).ok("pairing.create", json!({})));
    let id = invitation(&owned["status"]);
    let before = fixture.registry.pending_pairings().unwrap();
    blocking(|| {
        let mut client = ProductClient::connect(address, &member);
        for (method, params) in [
            ("pairing.create", json!({})),
            ("pairing.pending", json!({})),
            ("pairing.status", json!({"invitationId": id})),
            ("pairing.cancel", json!({"invitationId": id})),
            ("pairing.deny", json!({"invitationId": id})),
            (
                "pairing.approve",
                json!({"invitationId": id, "deviceKey": vec![7; 32]}),
            ),
        ] {
            assert_eq!(client.refused(method, params), "forbidden", "{method}");
        }
    });
    assert_eq!(fixture.registry.pending_pairings().unwrap(), before);
    let missing = blocking(|| {
        ProductClient::connect(address, &token)
            .refused("pairing.status", json!({"invitationId": vec![9; 16]}))
    });
    assert_eq!(missing, "pairing_not_found");
}

/// Row O4: approval names the exact claimed key; another key is a conflict
/// that changes nothing, and the claimed key is carried through to Active
/// (slice 2b). Deny of a claimed enrollment ends it as the owner's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_approve_exact_claim_before_effect() {
    let fixture = Fixture::new().await;
    let address = serve(&fixture, true).await;
    let (native, stop, listener, _) = fixture.listener().await;
    let token = fixture.owner_token.clone();
    for deny in [false, true] {
        let created =
            blocking(|| ProductClient::connect(address, &token).ok("pairing.create", json!({})));
        let id = invitation(&created["status"]);
        let (_, store) = pending(
            fixture.directory.path(),
            if deny {
                "client-deny"
            } else {
                "client-approve"
            },
        );
        let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
        let code = ManualCode::parse(created["code"].as_str().unwrap().as_bytes()).unwrap();
        let claimed = tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(native).unwrap(), code, OsEntropy),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(claimed, NativePairingStatus::Claimed(_)));
        blocking(|| {
            let mut owner = ProductClient::connect(address, &token);
            let status = owner.ok("pairing.status", json!({"invitationId": id}));
            assert_eq!(status["phase"], "claimed");
            let key: Vec<u8> = serde_json::from_value(status["claimedDeviceKey"].clone()).unwrap();
            if deny {
                let denied = owner.ok("pairing.deny", json!({"invitationId": id}));
                assert_eq!(denied["phase"], "terminal");
                assert_eq!(denied["terminal"]["cause"], "denied");
                assert_eq!(
                    denied["terminal"]["initiator"],
                    json!({"kind": "principal", "principalId": "owner"})
                );
                return;
            }
            let mut other = key.clone();
            other[0] ^= 1;
            assert_eq!(
                owner.refused(
                    "pairing.approve",
                    json!({"invitationId": id, "deviceKey": other})
                ),
                "pairing_conflict"
            );
            assert_eq!(
                owner.ok("pairing.status", json!({"invitationId": id}))["phase"],
                "claimed",
                "a conflicting approval changes nothing"
            );
            let approved = owner.ok(
                "pairing.approve",
                json!({"invitationId": id, "deviceKey": key}),
            );
            assert!(approved.get("activationStopped").is_none(), "{approved}");
            let approved = approved["status"].clone();
            // Approval carries the exact key through to an issued credential
            // and its paired receiver (slice 2b, rows A1, A2).
            assert_eq!(approved["phase"], "active");
            assert_eq!(approved["claimedDeviceKey"], json!(key));
            assert!(approved["credentialId"].is_string());
            assert!(approved["receiver"]["receiverId"].is_string());
            assert_eq!(approved["receiver"]["accessEpoch"], 1);
            assert_eq!(approved["cleanupPending"], false);
        });
        client.shutdown().await;
    }
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
}

/// Row O3: another administrator of the same organization cannot tell another
/// owner's invitation from an unknown one, and cannot change it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_hides_another_owners_invitation() {
    let fixture = Fixture::new().await;
    // A second admin principal in the same organization, with the same grants.
    let IssueCredentialOutcome::Issued { evidence, .. } = fixture
        .registry
        .provision_surface(
            "second-owner",
            "second-owner-request".into(),
            "second-owner-credential".into(),
            vec!["credential.manage".into(), "conversation.read".into()],
            100,
            None,
        )
        .unwrap()
    else {
        panic!("a new admin surface credential is issued");
    };
    let second = String::from_utf8(evidence.expose_bytes().to_vec()).unwrap();
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    let owned =
        blocking(|| ProductClient::connect(address, &token).ok("pairing.create", json!({})));
    let id = invitation(&owned["status"]);
    let before = fixture.registry.pending_pairings().unwrap();
    blocking(|| {
        let mut other = ProductClient::connect(address, &second);
        for (method, params) in [
            ("pairing.status", json!({"invitationId": id})),
            ("pairing.cancel", json!({"invitationId": id})),
            ("pairing.deny", json!({"invitationId": id})),
            (
                "pairing.approve",
                json!({"invitationId": id, "deviceKey": vec![7; 32]}),
            ),
        ] {
            assert_eq!(
                other.refused(method, params.clone()),
                other.refused(method, {
                    let mut unknown = params;
                    unknown["invitationId"] = json!(vec![9; 16]);
                    unknown
                }),
                "{method}: another owner's invitation must read as unknown"
            );
        }
        assert_eq!(
            other.refused("pairing.status", json!({"invitationId": id})),
            "pairing_not_found"
        );
        // Listing shows only the caller's own enrollments.
        assert_eq!(other.ok("pairing.pending", json!({}))["items"], json!([]));
    });
    assert_eq!(fixture.registry.pending_pairings().unwrap(), before);
}

/// Row O3: an administrator whose credential does not allow what the consent
/// grants (`conversation.read`) passes socket admission and is refused by
/// Auth's exact consent check, as `forbidden`, with nothing created.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_without_the_consent_grant_is_forbidden() {
    let fixture = Fixture::with_read(false).await;
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    blocking(|| {
        let mut owner = ProductClient::connect(address, &token);
        assert_eq!(owner.refused("pairing.create", json!({})), "forbidden");
        // Listing asks the same check per record; with none, nothing is shown.
        assert_eq!(owner.ok("pairing.pending", json!({}))["items"], json!([]));
    });
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
}

/// Row W1: malformed params are refused by the generated DTO before the
/// runtime is asked, and nothing is created.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_refuses_malformed_params_before_the_store() {
    let fixture = Fixture::new().await;
    let address = serve(&fixture, true).await;
    let token = fixture.owner_token.clone();
    blocking(|| {
        let mut owner = ProductClient::connect(address, &token);
        for (method, params) in [
            ("pairing.create", json!({"code": "ABCD-2345"})),
            ("pairing.pending", json!({"all": true})),
            ("pairing.status", json!({"invitationId": vec![1; 15]})),
            ("pairing.status", json!({"invitationId": vec![1; 17]})),
            ("pairing.status", json!({"invitationId": vec![256; 16]})),
            ("pairing.status", json!({})),
            (
                "pairing.cancel",
                json!({"invitationId": vec![1; 16], "reason": "x"}),
            ),
            (
                "pairing.approve",
                json!({"invitationId": vec![1; 16], "deviceKey": vec![1; 31]}),
            ),
            ("pairing.approve", json!({"invitationId": vec![1; 16]})),
        ] {
            assert_eq!(
                owner.refused(method, params.clone()),
                "invalid_request",
                "{method} {params}"
            );
        }
    });
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
}

/// Row S1 (route side): without native pairing every pairing method answers
/// `pairing_not_configured`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_without_native_pairing_is_not_configured() {
    let fixture = Fixture::new().await;
    let address = serve(&fixture, false).await;
    let token = fixture.owner_token.clone();
    blocking(|| {
        let mut owner = ProductClient::connect(address, &token);
        for (method, params) in [
            ("pairing.create", json!({})),
            ("pairing.pending", json!({})),
            ("pairing.status", json!({"invitationId": vec![1; 16]})),
            ("pairing.cancel", json!({"invitationId": vec![1; 16]})),
        ] {
            assert_eq!(owner.refused(method, params), "pairing_not_configured");
        }
    });
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
}

/// Row A11: approval's typed stop reaches the wire, `retryable` for a
/// receiver whose answer was lost (approving again then reaches active) and
/// `permanent` for a receiver no longer holding the pairing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_route_reports_activation_stops_on_the_wire() {
    for retryable in [true, false] {
        let (fixture, receivers) = crate::activation::fixture().await;
        let address = serve(&fixture, true).await;
        let (native, stop, listener, _) = fixture.listener().await;
        let token = fixture.owner_token.clone();
        let created =
            blocking(|| ProductClient::connect(address, &token).ok("pairing.create", json!({})));
        let id = invitation(&created["status"]);
        let (_, store) = pending(fixture.directory.path(), "client");
        let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
        let code = ManualCode::parse(created["code"].as_str().unwrap().as_bytes()).unwrap();
        tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(native).unwrap(), code, OsEntropy),
        )
        .await
        .unwrap()
        .unwrap();
        if retryable {
            receivers.lose_pair_answer.store(true, Ordering::SeqCst);
        } else {
            receivers.revoke_after_pair.store(true, Ordering::SeqCst);
        }
        blocking(|| {
            let mut owner = ProductClient::connect(address, &token);
            let key =
                owner.ok("pairing.status", json!({"invitationId": id}))["claimedDeviceKey"].clone();
            let approve = json!({"invitationId": id, "deviceKey": key});
            let stopped = owner.ok("pairing.approve", approve.clone());
            assert_eq!(stopped["status"]["phase"], "staging", "{stopped}");
            assert_eq!(
                stopped["activationStopped"],
                if retryable { "retryable" } else { "permanent" }
            );
            let again = owner.ok("pairing.approve", approve);
            if retryable {
                assert_eq!(again["status"]["phase"], "active", "{again}");
                assert!(again.get("activationStopped").is_none());
            } else {
                assert_eq!(again["activationStopped"], "permanent", "{again}");
            }
        });
        client.shutdown().await;
        stop.send(()).unwrap();
        listener.await.unwrap().unwrap();
    }
}
