use nessa_gateway_endpoint::{
    application::{DiscoverGatewayEndpoint, ManagedRuntimeAdvertisement, PublishGatewayEndpoint},
    domain::{EndpointIdentity, GatewayEndpoint},
    infrastructure::{FileEndpointDiscovery, FileEndpointPublication, ENDPOINT_FILE},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn endpoint(port: u16) -> GatewayEndpoint {
    GatewayEndpoint::new(
        format!("127.0.0.1:{port}").parse().unwrap(),
        EndpointIdentity::new("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd".into(), 909).unwrap(),
    )
    .unwrap()
}

#[test]
fn canonical_publication_agrees_with_the_cross_runtime_fixture() {
    let fixture = include_str!("../../../../protocol/fixtures/gateway-endpoint.json");
    let temporary = tempfile::tempdir().unwrap();
    let logs = temporary.path().join("logs");
    let publication = FileEndpointPublication::new(logs.clone());
    let endpoint = GatewayEndpoint::new(
        "127.0.0.1:9137".parse().unwrap(),
        EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711).unwrap(),
    )
    .unwrap();
    let managed = ManagedRuntimeAdvertisement::new(
        "a".repeat(64),
        "b".repeat(64),
        endpoint.identity().instance().into(),
        endpoint.identity().process_id(),
        endpoint.identity(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication)
        .execute(&endpoint, Some(&managed))
        .unwrap();
    assert_eq!(
        std::fs::read(logs.join(ENDPOINT_FILE)).unwrap(),
        fixture.trim_end().as_bytes()
    );
}

fn health_server(
    instance: &str,
    process_id: u32,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    health_server_with_managed(instance, process_id, None)
}

fn health_server_with_managed(
    instance: &str,
    process_id: u32,
    managed: Option<(&str, &str)>,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let instance = instance.to_owned();
    let managed = managed.map(|(fingerprint, generation)| {
        format!(
            "x-nessa-runtime-fingerprint: {fingerprint}\r\nx-nessa-service-generation: {generation}\r\nx-nessa-runtime-instance: {instance}\r\nx-nessa-process-id: {process_id}\r\n"
        )
    });
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let read = stream.read(&mut request).unwrap();
        assert!(std::str::from_utf8(&request[..read])
            .unwrap()
            .starts_with("GET /health "));
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nx-nessa-endpoint-instance: {instance}\r\nx-nessa-endpoint-process-id: {process_id}\r\n{}content-length: 0\r\n\r\n",
            managed.as_deref().unwrap_or_default()
        )
        .unwrap();
    });
    (address, server)
}

#[test]
fn managed_discovery_correlates_every_identity_field_over_a_real_health_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = temporary.path().join("logs");
    let instance = "3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd";
    let fingerprint = "a".repeat(64);
    let generation = "b".repeat(64);
    let (address, server) =
        health_server_with_managed(instance, 909, Some((&fingerprint, &generation)));
    let endpoint = GatewayEndpoint::new(
        address,
        EndpointIdentity::new(instance.into(), 909).unwrap(),
    )
    .unwrap();
    let managed = ManagedRuntimeAdvertisement::new(
        fingerprint,
        generation,
        instance.into(),
        909,
        endpoint.identity(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&FileEndpointPublication::new(logs.clone()))
        .execute(&endpoint, Some(&managed))
        .unwrap();
    assert_eq!(
        DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(logs))
            .execute()
            .unwrap()
            .unwrap()
            .address(),
        address
    );
    server.join().unwrap();
}

#[test]
fn publication_atomically_replaces_the_previous_bound_port_and_managed_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = temporary.path().join("logs");
    let adapter = FileEndpointPublication::new(logs.clone());
    PublishGatewayEndpoint::new(&adapter)
        .execute(&endpoint(7421), None)
        .unwrap();
    let replaced = endpoint(8137);
    let managed = ManagedRuntimeAdvertisement::new(
        "a".repeat(64),
        "b".repeat(64),
        replaced.identity().instance().into(),
        replaced.identity().process_id(),
        replaced.identity(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&adapter)
        .execute(&replaced, Some(&managed))
        .unwrap();

    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(logs.join(ENDPOINT_FILE)).unwrap()).unwrap();
    assert_eq!(record["webSocketUrl"], "ws://127.0.0.1:8137");
    assert_eq!(
        record["endpointInstance"],
        "3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd"
    );
    assert_eq!(record["processId"], 909);
    assert_eq!(record["runtimeFingerprint"], "a".repeat(64));
    assert_eq!(record["serviceGeneration"], "b".repeat(64));
    assert_eq!(
        record["runtimeInstance"],
        "3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd"
    );
    assert_eq!(record["runtimeProcessId"], 909);
}

#[test]
fn managed_identity_must_be_the_same_process_as_the_endpoint() {
    let endpoint = endpoint(8137);
    assert!(ManagedRuntimeAdvertisement::new(
        "a".repeat(64),
        "b".repeat(64),
        "78f4377b-e600-4f4c-94eb-99ad6f35a62e".into(),
        endpoint.identity().process_id(),
        endpoint.identity(),
    )
    .is_err());
    assert!(ManagedRuntimeAdvertisement::new(
        "a".repeat(64),
        "b".repeat(64),
        endpoint.identity().instance().into(),
        910,
        endpoint.identity(),
    )
    .is_err());
}

#[test]
fn discovery_reads_a_real_private_file_and_correlates_a_real_health_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = temporary.path().join("logs");
    let (address, server) = health_server("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd", 909);
    let publication = FileEndpointPublication::new(logs.clone());
    PublishGatewayEndpoint::new(&publication)
        .execute(
            &GatewayEndpoint::new(address, endpoint(address.port()).identity().clone()).unwrap(),
            None,
        )
        .unwrap();
    let discovered = DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(logs))
        .execute()
        .unwrap()
        .unwrap();
    assert_eq!(discovered.web_socket_url(), format!("ws://{address}"));
    server.join().unwrap();
}

#[test]
fn absent_record_falls_back_but_mismatched_identity_is_refused() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = temporary.path().join("logs");
    assert!(
        DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(logs.clone()))
            .execute()
            .unwrap()
            .is_none()
    );

    let (address, server) = health_server("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd", 910);
    let publication = FileEndpointPublication::new(logs.clone());
    PublishGatewayEndpoint::new(&publication)
        .execute(
            &GatewayEndpoint::new(address, endpoint(address.port()).identity().clone()).unwrap(),
            None,
        )
        .unwrap();
    let error = DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(logs))
        .execute()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    server.join().unwrap();
}
