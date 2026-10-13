//! Empty-ledger signed setup through the real authenticated product socket.
use super::*;
use nessa_auth::{
    adapters::pairing::NativeIdentity, application::pairing::PrivateKeyMaterial,
    domain::pairing::OwnerPhoneRequestId,
};
use nessa_protocol::product::{
    generated::{PhoneAuthorizeParams, PhoneDiscovery},
    owner_phone,
};
use serde_json::{json, Value};
use zeroize::Zeroizing;

#[path = "../device_pairing/product_client.rs"]
pub(super) mod client;
use client::ProductClient;

fn credential(registry: &LocalCredentialStore) -> String {
    let issued = registry
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            credential_id: "phone-root".into(),
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member".into(),
                principal_id: "owner".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            issued_at: SystemClock.unix_seconds() - 1,
            expires_at: None,
            grants: [
                "server.read",
                "credential.manage",
                "conversation.read",
                "conversation.write",
            ]
            .into_iter()
            .map(|action| CredentialGrantDto {
                action: action.into(),
                resource: ResourceDto {
                    organization_id: "org".into(),
                    id: "gateway".into(),
                },
            })
            .collect(),
        })
        .unwrap();
    String::from_utf8(issued.evidence.expose_bytes().to_vec()).unwrap()
}

fn signed(client: &mut ProductClient, nonce: u8) -> Value {
    let discovery: PhoneDiscovery =
        serde_json::from_value(client.ok("phone.endpoint", json!({}))).unwrap();
    let context = owner_phone::context(discovery).unwrap();
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([nonce; 32]))).unwrap();
    let request = context
        .request(
            OwnerPhoneRequestId::new([nonce; 16]).unwrap(),
            identity.device_key(),
        )
        .unwrap();
    serde_json::to_value(PhoneAuthorizeParams {
        original: owner_phone::original(&request).unwrap(),
        signature: identity
            .sign_owner_phone_request(&request)
            .unwrap()
            .into_iter()
            .map(u64::from)
            .collect(),
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signed_setup_from_empty_ledger_activates_once_and_reads_exact_original() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let token = credential(&registry);
    let (prepared, commands, _peers) = prepare(&loopback(), namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands);
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let serving = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap();
    });
    assert!(
        registry.pending_pairings().unwrap().is_empty(),
        "positive must begin without seeded consent"
    );
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &token);
        let params = signed(&mut client, 61);
        let registry_path = namespace.root().join("auth/credentials.json");
        let before = std::fs::read(&registry_path).unwrap();
        for malformed in [
            json!([]),
            json!(null),
            json!([params["original"], params["signature"]]),
        ] {
            assert_eq!(
                client.refused("phone.authorize", malformed),
                "invalid_request"
            );
        }
        assert!(
            std::fs::read(&registry_path).unwrap() == before,
            "malformed input leaves the complete registry revision and bytes unchanged"
        );
        let initial = client.ok("phone.authorize", params.clone());
        assert_eq!(initial["phase"], "active", "{initial}");
        assert_eq!(initial["original"], params["original"]);
        assert!(initial["credentialId"].is_string());
        assert!(initial["receiver"]["accessEpoch"].is_u64());
        let replay = client.ok("phone.authorize", params);
        assert_eq!(
            replay, initial,
            "same original must not replace invitation, stage or receiver"
        );
        let read = client.ok(
            "phone.status",
            json!({ "requestId": initial["original"]["requestId"],
            "originalDigest": initial["originalDigest"] }),
        );
        assert_eq!(read, initial);
        let mut changed = initial["originalDigest"].clone();
        changed[0] = json!((changed[0].as_u64().unwrap() + 1) % 256);
        assert_eq!(
            client.refused(
                "phone.status",
                json!({ "requestId": initial["original"]["requestId"],
            "originalDigest": changed })
            ),
            "pairing_conflict"
        );
    });
    assert!(
        registry.pending_pairings().unwrap().is_empty(),
        "completed setup leaves no unfinished enrollment"
    );
    serving.abort();
    let _ = serving.await;
    drop(bound);
}

#[path = "owner_phone_expiry.rs"]
mod expiry;
#[path = "owner_phone_route_refusals.rs"]
mod refusals;

#[path = "owner_phone_late_pair.rs"]
mod late_pair;
#[path = "owner_phone_recovery.rs"]
mod recovery;

#[path = "owner_phone_method_admission.rs"]
mod method_admission;
