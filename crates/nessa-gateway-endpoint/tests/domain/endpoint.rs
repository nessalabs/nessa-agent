use nessa_gateway_endpoint::domain::{
    EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement, ManagedRuntimeIdentity,
};

fn identity() -> EndpointIdentity {
    EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 123).unwrap()
}

#[test]
fn identity_requires_a_canonical_process_incarnation() {
    assert!(EndpointIdentity::new("not-a-uuid".into(), 1).is_err());
    assert!(EndpointIdentity::new("7A653268-43FC-4E76-A4D3-DF749CC629B1".into(), 1).is_err());
    assert!(EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 0).is_err());
}

#[test]
fn endpoint_accepts_only_a_bound_loopback_socket() {
    let loopback = GatewayEndpoint::new("ws://127.0.0.1:9123".into(), identity()).unwrap();
    assert_eq!(loopback.web_socket_url(), "ws://127.0.0.1:9123");
    assert!(GatewayEndpoint::new("ws://127.0.0.1:0".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://192.0.2.1:9123".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://127.0.0.2:9123".into(), identity()).is_err());
    assert!(GatewayEndpoint::new("ws://[::1]:9123".into(), identity()).is_ok());
}

#[test]
fn advertisement_owns_the_endpoint_and_managed_identity_relationship() {
    let endpoint = GatewayEndpoint::new("ws://127.0.0.1:9123".into(), identity()).unwrap();
    let matching = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        identity().instance().into(),
        identity().process_id(),
    )
    .unwrap();
    assert!(GatewayEndpointAdvertisement::new(endpoint.clone(), Some(matching)).is_ok());

    let mismatched = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        "5fe20b6f-21bd-43cc-a050-8a8ead480755".into(),
        456,
    )
    .unwrap();
    assert!(GatewayEndpointAdvertisement::new(endpoint, Some(mismatched)).is_err());
    assert!(ManagedRuntimeIdentity::new(
        "not-a-digest".into(),
        "b".repeat(64),
        identity().instance().into(),
        identity().process_id(),
    )
    .is_err());
}
