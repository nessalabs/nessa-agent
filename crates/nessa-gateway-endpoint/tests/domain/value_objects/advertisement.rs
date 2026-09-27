use nessa_gateway_endpoint::domain::{
    EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement, ManagedRuntimeIdentity,
};

fn identity() -> EndpointIdentity {
    EndpointIdentity::new("7a653268-43fc-4e76-a4d3-df749cc629b1".into(), 123).unwrap()
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
