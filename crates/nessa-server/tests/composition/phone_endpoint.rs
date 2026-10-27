//! Actual native binding and authenticated socket discovery, without enrollment.
use super::*;
use serde_json::json;
use std::net::IpAddr;
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
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = NativeConfig {
        listen_address: listener.local_addr().unwrap(),
        relay: None,
        advertised_address: None,
        tunnel: None,
    };
    let (prepared, commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let prepared = adopt(prepared, listener);
    assert!(
        commands.native_endpoint().is_none(),
        "publication waits until the listener is bound"
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

#[test]
fn a_wildcard_with_a_bound_port_uses_the_route_and_port_zero_stays_unpublished() {
    let route = "192.0.2.10".parse().unwrap();
    assert_eq!(
        super::super::advertised_endpoint("0.0.0.0:47650".parse().unwrap(), Some(route)),
        Some("192.0.2.10:47650".parse().unwrap())
    );
    for (bound, route) in [
        ("0.0.0.0:0", Some(route)),
        ("127.0.0.1:0", None),
        ("0.0.0.0:47650", None),
        ("0.0.0.0:47650", Some("127.0.0.1".parse().unwrap())),
        ("[::]:47650", Some(route)),
        ("[::]:47650", Some("fe80::1".parse().unwrap())),
        ("0.0.0.0:47650", Some("0.0.0.1".parse().unwrap())),
        ("0.0.0.0:47650", Some("255.255.255.255".parse().unwrap())),
    ] {
        assert_eq!(
            super::super::advertised_endpoint(bound.parse().unwrap(), route),
            None,
            "{bound}"
        );
    }
    assert_eq!(
        super::super::advertised_endpoint("127.0.0.1:47650".parse().unwrap(), Some(route)),
        Some("127.0.0.1:47650".parse().unwrap())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_refuses_absent_and_unbound_native_destinations() {
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
    let config = NativeConfig {
        listen_address: "0.0.0.0:0".parse().unwrap(),
        relay: None,
        advertised_address: Some("192.0.2.10".parse().unwrap()),
        tunnel: None,
    };
    let (prepared, commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let commands = Arc::new(commands);
    let state = product(registry).with_pairing(commands.clone());
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    assert_ne!(bound.local_address().port(), 0);
    assert!(commands.native_endpoint().is_none());
    let (address, serving) = serve(state).await;
    tokio::task::block_in_place(|| {
        assert_eq!(
            ProductClient::connect(address, &credential).refused("phone.endpoint", json!({})),
            "pairing_unavailable"
        );
    });
    assert!(commands.native_endpoint().is_none());
    serving.abort();
    let _ = serving.await;
    drop(bound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn endpoint_publishes_the_configured_address_for_a_wildcard_bind() {
    let namespace = Namespace::new();
    let registry = namespace.registry();
    let credential = token(&registry, true);
    let advertised = "192.0.2.10".parse().unwrap();
    let listener = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let config = NativeConfig {
        listen_address: listener.local_addr().unwrap(),
        relay: None,
        advertised_address: Some(advertised),
        tunnel: None,
    };
    let (prepared, commands, _peers) = prepare(&config, namespace.inputs(registry.clone()))
        .await
        .unwrap();
    let prepared = adopt(prepared, listener);
    let commands = Arc::new(commands);
    let state = product(registry).with_pairing(commands.clone());
    let bound = bind(
        prepared,
        RuntimeDependencies::default().clock,
        state.clone(),
    )
    .await
    .unwrap();
    let listener = bound.local_address();
    assert!(listener.ip().is_unspecified());
    assert_ne!(listener.port(), 0);
    let expected = commands.native_endpoint().unwrap();
    assert_eq!(expected.0, SocketAddr::new(advertised, listener.port()));
    assert!(!expected.0.ip().is_unspecified());
    assert!(!expected.0.ip().is_loopback());
    let (address, serving) = serve(state).await;
    tokio::task::block_in_place(|| {
        let found = ProductClient::connect(address, &credential).ok("phone.endpoint", json!({}));
        assert_eq!(found["nativeEndpoint"], expected.0.to_string());
        assert_eq!(found["gatewayKey"], json!(expected.1.to_vec()));
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
    let (prepared, commands, _peers) = prepared_loopback(namespace.inputs(registry.clone())).await;
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

#[test]
fn concrete_and_wildcard_binds_share_the_auth_endpoint_rule() {
    for (host, accepted) in [
        ("192.0.2.10", true),
        ("::ffff:192.0.2.10", true),
        ("::ffff:0.0.0.1", false),
        ("::ffff:224.0.0.1", false),
        ("10.0.0.1", true),
        ("2001:db8::1", true),
        ("fd00::1", true),
        ("0.0.0.1", false),
        ("255.255.255.255", false),
        ("224.0.0.1", false),
        ("ff02::1", false),
        ("fe80::1", false),
        ("febf::1", false),
    ] {
        let ip: IpAddr = host.parse().unwrap();
        let concrete = SocketAddr::new(ip, 47650);
        let wildcard = if ip.is_ipv4() {
            "0.0.0.0:47650"
        } else {
            "[::]:47650"
        }
        .parse()
        .unwrap();
        for (bound, advertised) in [(concrete, None), (wildcard, Some(ip))] {
            assert_eq!(
                super::super::advertised_endpoint(bound, advertised),
                accepted.then_some(concrete),
                "{bound}, {advertised:?}"
            );
        }
        assert_eq!(
            super::super::advertised_endpoint(SocketAddr::new(ip, 0), None),
            None
        );
    }
    assert_eq!(
        super::super::advertised_endpoint("[fe80::1%3]:47650".parse().unwrap(), None),
        None
    );
    let loopback = "[::ffff:127.0.0.1]:47650".parse().unwrap();
    assert_eq!(
        super::super::advertised_endpoint(loopback, None),
        Some(loopback)
    );
    assert_eq!(
        super::super::advertised_endpoint(
            "[::]:47650".parse().unwrap(),
            Some("::ffff:127.0.0.1".parse().unwrap())
        ),
        None
    );
}
