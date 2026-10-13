//! Actual native binding and authenticated socket discovery, without enrollment.
use super::*;
use serde_json::json;
use tokio::task::JoinHandle;

use super::owner_phone::client::ProductClient;

fn token(registry: &LocalCredentialStore, read: bool) -> String {
    let issued = registry
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            credential_id: format!("endpoint-credential-{read}"),
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
                "credential.manage",
                if read {
                    "server.read"
                } else {
                    "conversation.read"
                },
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

async fn serve(state: ProductRouteState) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, crate::server::entrypoint::http::router(state))
            .await
            .unwrap();
    });
    (address, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_discloses_actual_bound_port_and_current_source_without_allocating_enrollment() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let credential = token(&registry, true);
    let (prepared, commands, _peers) = prepare(&loopback(), namespace.inputs(registry.clone()))
        .await
        .unwrap();
    assert!(
        commands.native_endpoint().is_none(),
        "configured port zero is not discovery"
    );
    let commands = Arc::new(commands);
    let state = product(registry.clone()).with_pairing(commands.clone());
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let expected = commands.native_endpoint().unwrap();
    assert_eq!(expected.0, bound.local_address());
    assert_ne!(expected.0.port(), 0);
    let before = registry.pending_pairings().unwrap();
    let registry_path = namespace.root().join("auth/credentials.json");
    let registry_before = std::fs::read(&registry_path).unwrap();
    let (address, serving) = serve(state).await;
    tokio::task::block_in_place(|| {
        let mut client = ProductClient::connect(address, &credential);
        let first = client.ok("phone.endpoint", json!({}));
        assert_eq!(first["nativeEndpoint"], expected.0.to_string());
        assert_eq!(first["gatewayKey"], json!(expected.1.to_vec()));
        assert_eq!(
            first["source"],
            json!({
                "ownerId": "owner", "organizationId": "org", "membershipId": "member",
                "audience": "gateway", "credentialId": "endpoint-credential-true", "resourceId": "gateway"
            })
        );
        let discovery = serde_json::from_value(first.clone()).unwrap();
        assert!(nessa_protocol::product::owner_phone::context(discovery).is_ok());
        assert_eq!(client.ok("phone.endpoint", json!({})), first);
        for params in [
            json!([]),
            json!(null),
            json!(""),
            json!({"nativeEndpoint": "127.0.0.1:1"}),
        ] {
            assert_eq!(client.refused("phone.endpoint", params), "invalid_request");
        }
    });
    assert_eq!(
        registry.pending_pairings().unwrap(),
        before,
        "passive repeats allocate nothing"
    );
    assert!(
        std::fs::read(&registry_path).unwrap() == registry_before,
        "discovery changes no registry history, including active records"
    );
    serving.abort();
    let _ = serving.await;
    drop(bound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_refuses_absent_unbound_and_wildcard_native_destinations() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let credential = token(&registry, true);
    let (address, serving) = serve(product(registry.clone())).await;
    tokio::task::block_in_place(|| {
        assert_eq!(
            ProductClient::connect(address, &credential).refused("phone.endpoint", json!({})),
            "pairing_not_configured"
        );
    });
    serving.abort();
    let _ = serving.await;
    let mut config = loopback();
    config.listen_address = "0.0.0.0:0".parse().unwrap();
    let (prepared, commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let commands = Arc::new(commands);
    let state = product(registry).with_pairing(commands.clone());
    let (address, serving) = serve(state.clone()).await;
    tokio::task::block_in_place(|| {
        assert_eq!(
            ProductClient::connect(address, &credential).refused("phone.endpoint", json!({})),
            "pairing_unavailable"
        );
    });
    let bound = bind(prepared, RuntimeDependencies::default().clock, state)
        .await
        .unwrap();
    assert_ne!(bound.local_address().port(), 0);
    assert!(commands.native_endpoint().is_none());
    tokio::task::block_in_place(|| {
        assert_eq!(
            ProductClient::connect(address, &credential).refused("phone.endpoint", json!({})),
            "pairing_unavailable"
        );
    });
    serving.abort();
    let _ = serving.await;
    drop(bound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_asks_current_socket_policy_before_native_disclosure() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let credential = token(&registry, false);
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
    let before = registry.pending_pairings().unwrap();
    let (address, serving) = serve(state).await;
    tokio::task::block_in_place(|| {
        assert_eq!(
            ProductClient::connect(address, &credential).refused("phone.endpoint", json!({})),
            "forbidden"
        );
    });
    assert_eq!(registry.pending_pairings().unwrap(), before);
    serving.abort();
    let _ = serving.await;
    drop(bound);
}
