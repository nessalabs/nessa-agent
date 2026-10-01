//! Private command inputs and current publication use the real file owners.
use super::{Profile, ProfileError, MAX_PROFILE_BYTES};
use nessa_gateway_endpoint::{
    application::PublishGatewayEndpoint,
    domain::{EndpointIdentity, GatewayEndpoint, GatewayEndpointAdvertisement},
    infrastructure::FileEndpointPublication,
};
use nessa_local_storage::OpenMode;
use serde_json::json;
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt};
use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn write(path: &Path, bytes: &[u8]) {
    nessa_local_storage::open(path, OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn document(root: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({"receiver":"receiver", "accessEpoch":7,
        "credentialFile":root.join("reader.token"), "endpointRoot":root,
        "endpointDirectory":"gateway"}))
    .unwrap()
}

fn configure_health_socket(socket: &TcpStream) {
    // Accepted sockets may inherit the listener's mode. Timeouts bound blocking
    // I/O; they do not turn a nonblocking socket into a blocking one.
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
}

#[test]
fn health_socket_waits_for_controlled_request_after_nonblocking_accept() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    // Force the inherited mode on every platform, before any request exists.
    accepted.set_nonblocking(true).unwrap();
    let mut bytes = [0; 64];
    assert_eq!(
        accepted.read(&mut bytes).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    configure_health_socket(&accepted);
    let (ready_send, ready) = std::sync::mpsc::channel();
    let (result_send, result) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        ready_send.send(()).unwrap();
        let read = accepted
            .read(&mut bytes)
            .map(|count| bytes[..count].to_vec());
        result_send.send(read).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
    let before_request = result.recv_timeout(Duration::from_millis(50));
    client.write_all(b"GET /health HTTP/1.1\r\n\r\n").unwrap();
    reader.join().unwrap();
    assert!(
        matches!(
            before_request,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "the fixture read must wait for the controlled request: {before_request:?}"
    );
    let request = result
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert!(request.starts_with(b"GET /health "));
}

#[test]
fn private_profile_admission_is_bounded_and_explicit() {
    let root = tempfile::tempdir().unwrap();
    let valid = document(root.path());
    let path = root.path().join("valid.json");
    let mut exact = valid.clone();
    exact.resize(MAX_PROFILE_BYTES, b' ');
    write(&path, &exact);
    let profile = Profile::load(&path).unwrap();
    assert_eq!(profile.receiver.as_str(), "receiver");
    assert_eq!(profile.access_epoch, 7);
    assert_eq!(profile.credential_file, root.path().join("reader.token"));
    assert_eq!(profile.endpoint_root, root.path());
    assert_eq!(profile.endpoint_directory, PathBuf::from("gateway"));
    let oversized = root.path().join("oversized.json");
    exact.push(b' ');
    write(&oversized, &exact);
    assert!(matches!(
        Profile::load(&oversized),
        Err(ProfileError::TooLarge)
    ));
    assert!(matches!(
        Profile::load(&root.path().join("missing")),
        Err(ProfileError::Unavailable)
    ));
    let text = String::from_utf8(valid).unwrap();
    for (index, text) in [
        "{broken".to_owned(),
        text.replace("\"receiver\":\"receiver\"", "\"receiver\":\"\""),
        text.replace(
            "\"receiver\":\"receiver\"",
            "\"receiver\":\"receiver\",\"receiver\":\"other\"",
        ),
        text.replace(
            "\"receiver\":\"receiver\"",
            "\"receiver\":\"receiver\",\"unknown\":true",
        ),
        text.replace(
            &serde_json::to_string(&root.path().join("reader.token")).unwrap(),
            "\"relative.token\"",
        ),
        text.replace(
            &serde_json::to_string(root.path()).unwrap(),
            "\"relative-root\"",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.path().join(format!("invalid-{index}.json"));
        write(&path, text.as_bytes());
        assert!(
            matches!(Profile::load(&path), Err(ProfileError::Invalid)),
            "case {index}"
        );
    }
    // Parsing configuration has acquired neither credential nor endpoint files.
    assert!(!root.path().join("reader.token").exists());
    assert!(!root.path().join("gateway").exists());
}

#[test]
fn profile_resolves_current_endpoint_without_copied_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("private");
    nessa_local_storage::create_directory(&root).unwrap();
    let path = root.join("profile.json");
    write(&path, &document(&root));
    let profile = Profile::load(&path).unwrap();
    assert_eq!(profile.endpoint(), Err(ProfileError::EndpointUnavailable));
    let publication = FileEndpointPublication::new(root, "gateway".into());
    let listeners = [
        TcpListener::bind("127.0.0.1:0").unwrap(),
        TcpListener::bind("127.0.0.1:0").unwrap(),
    ];
    for listener in listeners {
        let endpoint = GatewayEndpoint::new(
            format!("ws://{}", listener.local_addr().unwrap()),
            EndpointIdentity::new(Uuid::new_v4().to_string(), std::process::id()).unwrap(),
        )
        .unwrap();
        PublishGatewayEndpoint::new(&publication)
            .execute(&GatewayEndpointAdvertisement::new(endpoint.clone(), None).unwrap())
            .unwrap();
        let identity = endpoint.identity().clone();
        listener.set_nonblocking(true).unwrap();
        let health = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    _ => return,
                }
            };
            configure_health_socket(&socket);
            let mut request = [0u8; 4096];
            let read = socket.read(&mut request).unwrap();
            assert!(request[..read].starts_with(b"GET /health "));
            write!(socket, "HTTP/1.1 200 OK\r\nX-Nessa-Endpoint-Instance: {}\r\nX-Nessa-Endpoint-Process-Id: {}\r\nContent-Length: 0\r\n\r\n", identity.instance(), identity.process_id()).unwrap();
        });
        let resolved = profile.endpoint();
        health.join().unwrap();
        assert_eq!(resolved.unwrap(), endpoint);
    }
}

#[test]
fn credential_acquisition_uses_supplied_wire_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("profile.json");
    write(&path, &document(root.path()));
    let mut profile = Profile::load(&path).unwrap();
    assert_eq!(
        profile.credential(4),
        Err(ProfileError::CredentialUnavailable)
    );
    for (index, bytes, expected) in [
        (0, "🦀🦀🦀🦀".as_bytes(), Ok("🦀🦀🦀🦀")),
        (1, b"secret\n".as_slice(), Ok("secret")),
        (
            2,
            b"0123456789abcdefg".as_slice(),
            Err(ProfileError::CredentialTooLarge),
        ),
        (3, b"\xff".as_slice(), Err(ProfileError::CredentialEncoding)),
    ] {
        profile.credential_file = root.path().join(format!("credential-{index}"));
        write(&profile.credential_file, bytes);
        // Physical acquisition permits borrowed text for the session's semantic check.
        assert_eq!(
            profile.credential(4).as_deref().map_err(|error| *error),
            expected
        );
    }
    assert_eq!(
        profile.credential(usize::MAX),
        Err(ProfileError::CredentialTooLarge)
    );
}

#[cfg(unix)]
#[test]
fn profile_and_credential_require_private_file_owner() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("profile.json");
    write(&private, &document(root.path()));
    let profile = Profile::load(&private).unwrap();
    write(&profile.credential_file, b"secret");
    std::fs::set_permissions(&profile.credential_file, Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        profile.credential(4),
        Err(ProfileError::CredentialUnavailable)
    );
    std::fs::set_permissions(&private, Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        Profile::load(&private),
        Err(ProfileError::Unavailable)
    ));
}
