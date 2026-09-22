use nessa_gateway_endpoint::{
    application::{DiscoverGatewayEndpoint, PublishGatewayEndpoint},
    domain::{
        EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement, ManagedRuntimeIdentity,
    },
    infrastructure::{FileEndpointDiscovery, FileEndpointPublication, ENDPOINT_FILE},
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn private_logs(temporary: &TempDir) -> PathBuf {
    let root = temporary.path().join("private-data");
    nessa_local_storage::create_directory(&root).unwrap();
    root.join("logs")
}

fn publication(directory: &Path) -> FileEndpointPublication {
    FileEndpointPublication::new(
        directory.parent().unwrap().to_path_buf(),
        directory.file_name().unwrap().into(),
    )
}

fn discovery(directory: &Path) -> FileEndpointDiscovery {
    FileEndpointDiscovery::new(
        directory.parent().unwrap().to_path_buf(),
        directory.file_name().unwrap().into(),
    )
}

fn advertisement(
    endpoint: &GatewayEndpoint,
    managed: Option<ManagedRuntimeIdentity>,
) -> GatewayEndpointAdvertisement {
    GatewayEndpointAdvertisement::new(endpoint.clone(), managed).unwrap()
}

fn endpoint(port: u16) -> GatewayEndpoint {
    GatewayEndpoint::new(
        format!("ws://127.0.0.1:{port}"),
        EndpointIdentity::new("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd".into(), 909).unwrap(),
    )
    .unwrap()
}

#[test]
fn canonical_publication_agrees_with_the_cross_runtime_fixture() {
    let fixture = include_str!("../../../../protocol/fixtures/gateway-endpoint.json");
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let publication = publication(&logs);
    let endpoint = GatewayEndpoint::new(
        "ws://127.0.0.1:9137".into(),
        EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711).unwrap(),
    )
    .unwrap();
    let managed = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        endpoint.identity().instance().into(),
        endpoint.identity().process_id(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication)
        .execute(&advertisement(&endpoint, Some(managed)))
        .unwrap();
    assert_eq!(
        std::fs::read(logs.join(ENDPOINT_FILE)).unwrap(),
        fixture.trim_end().as_bytes()
    );
}

fn health_server(instance: &str, process_id: u32) -> (SocketAddr, thread::JoinHandle<()>) {
    health_server_with_managed(instance, process_id, None)
}

fn health_server_with_managed(
    instance: &str,
    process_id: u32,
    managed: Option<(&str, &str)>,
) -> (SocketAddr, thread::JoinHandle<()>) {
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
    let logs = private_logs(&temporary);
    let instance = "3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd";
    let fingerprint = "a".repeat(64);
    let generation = "b".repeat(64);
    let (address, server) =
        health_server_with_managed(instance, 909, Some((&fingerprint, &generation)));
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        EndpointIdentity::new(instance.into(), 909).unwrap(),
    )
    .unwrap();
    let managed =
        ManagedRuntimeIdentity::new(fingerprint, generation, instance.into(), 909).unwrap();
    PublishGatewayEndpoint::new(&publication(&logs))
        .execute(&advertisement(&endpoint, Some(managed)))
        .unwrap();
    assert_eq!(
        DiscoverGatewayEndpoint::new(&discovery(&logs))
            .execute()
            .unwrap()
            .unwrap()
            .web_socket_url(),
        format!("ws://{address}")
    );
    server.join().unwrap();
}

#[test]
fn publication_atomically_replaces_the_previous_bound_port_and_managed_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let adapter = publication(&logs);
    PublishGatewayEndpoint::new(&adapter)
        .execute(&advertisement(&endpoint(7421), None))
        .unwrap();
    let replaced = endpoint(8137);
    let managed = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        replaced.identity().instance().into(),
        replaced.identity().process_id(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&adapter)
        .execute(&advertisement(&replaced, Some(managed)))
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
fn contradictory_managed_identity_cannot_be_published_or_write_a_file() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let endpoint = endpoint(8137);
    let another_instance = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        "78f4377b-e600-4f4c-94eb-99ad6f35a62e".into(),
        endpoint.identity().process_id(),
    )
    .unwrap();
    assert!(GatewayEndpointAdvertisement::new(endpoint.clone(), Some(another_instance)).is_err());
    let another_process = ManagedRuntimeIdentity::new(
        "a".repeat(64),
        "b".repeat(64),
        endpoint.identity().instance().into(),
        910,
    )
    .unwrap();
    assert!(GatewayEndpointAdvertisement::new(endpoint, Some(another_process)).is_err());
    assert!(!logs.join(ENDPOINT_FILE).exists());
}

#[test]
fn discovery_reads_a_real_private_file_and_correlates_a_real_health_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let (address, server) = health_server("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd", 909);
    let publication = publication(&logs);
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        endpoint(address.port()).identity().clone(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication)
        .execute(&advertisement(&endpoint, None))
        .unwrap();
    let discovered = DiscoverGatewayEndpoint::new(&discovery(&logs))
        .execute()
        .unwrap()
        .unwrap();
    assert_eq!(discovered.web_socket_url(), format!("ws://{address}"));
    server.join().unwrap();
}

#[test]
fn absent_record_falls_back_but_mismatched_identity_is_refused() {
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    assert!(DiscoverGatewayEndpoint::new(&discovery(&logs))
        .execute()
        .unwrap()
        .is_none());

    let (address, server) = health_server("3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd", 910);
    let publication = publication(&logs);
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        endpoint(address.port()).identity().clone(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication)
        .execute(&advertisement(&endpoint, None))
        .unwrap();
    let error = DiscoverGatewayEndpoint::new(&discovery(&logs))
        .execute()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    server.join().unwrap();
}

#[test]
fn a_domain_invalid_record_cannot_open_a_health_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    nessa_local_storage::create_directory(&logs).unwrap();
    let mut file = nessa_local_storage::open(
        &logs.join(ENDPOINT_FILE),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap();
    write!(
        file,
        "{{\"webSocketUrl\":\"ws://{address}/\",\"endpointInstance\":\"3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd\",\"processId\":909}}"
    )
    .unwrap();
    drop(file);

    let error = DiscoverGatewayEndpoint::new(&discovery(&logs))
        .execute()
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "domain-invalid endpoint must fail before unauthenticated health I/O"
    );
}

#[cfg(unix)]
#[test]
fn an_intermediate_symlink_cannot_redirect_endpoint_read_or_publication() {
    use std::os::unix::fs::symlink;

    let trusted = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), trusted.path().join("logs")).unwrap();

    let read = DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(
        trusted.path().to_path_buf(),
        "logs".into(),
    ))
    .execute()
    .unwrap_err();
    assert!(nessa_local_storage::is_unsafe_file(&read));

    let write = PublishGatewayEndpoint::new(&FileEndpointPublication::new(
        trusted.path().to_path_buf(),
        "logs".into(),
    ))
    .execute(&advertisement(&endpoint(9137), None))
    .unwrap_err();
    assert!(nessa_local_storage::is_unsafe_file(&write));
    assert!(!outside.path().join(ENDPOINT_FILE).exists());
}

#[cfg(unix)]
#[test]
fn a_permissive_root_is_refused_before_read_or_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("permissive-data");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();

    let read =
        DiscoverGatewayEndpoint::new(&FileEndpointDiscovery::new(root.clone(), "logs".into()))
            .execute()
            .unwrap_err();
    assert!(nessa_local_storage::is_unsafe_file(&read));

    let write =
        PublishGatewayEndpoint::new(&FileEndpointPublication::new(root.clone(), "logs".into()))
            .execute(&advertisement(&endpoint(9137), None))
            .unwrap_err();
    assert!(nessa_local_storage::is_unsafe_file(&write));
    assert!(!root.join("logs").exists());
}

#[test]
fn health_correlation_has_one_total_deadline_across_slow_reads() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        for byte in b"HTTP/1.1 200 OK".iter().take(8) {
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(150));
        }
    });
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        endpoint(address.port()).identity().clone(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication(&logs))
        .execute(&advertisement(&endpoint, None))
        .unwrap();

    let started = Instant::now();
    assert!(DiscoverGatewayEndpoint::new(&discovery(&logs))
        .execute()
        .is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    server.join().unwrap();
}

#[test]
fn bracketed_ipv6_loopback_is_discovered_over_a_real_health_socket() {
    let listener = match TcpListener::bind("[::1]:0") {
        Ok(listener) => listener,
        Err(_) => return,
    };
    let address = listener.local_addr().unwrap();
    let instance = "3f43acfb-3ce4-48fb-8dd1-d31c9404a6bd";
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);
        write!(stream, "HTTP/1.1 200 OK\r\nx-nessa-endpoint-instance: {instance}\r\nx-nessa-endpoint-process-id: 909\r\ncontent-length: 0\r\n\r\n").unwrap();
    });
    let temporary = tempfile::tempdir().unwrap();
    let logs = private_logs(&temporary);
    let endpoint = GatewayEndpoint::new(
        format!("ws://{address}"),
        EndpointIdentity::new(instance.into(), 909).unwrap(),
    )
    .unwrap();
    PublishGatewayEndpoint::new(&publication(&logs))
        .execute(&advertisement(&endpoint, None))
        .unwrap();
    assert_eq!(
        DiscoverGatewayEndpoint::new(&discovery(&logs))
            .execute()
            .unwrap()
            .unwrap(),
        endpoint
    );
    server.join().unwrap();
}
