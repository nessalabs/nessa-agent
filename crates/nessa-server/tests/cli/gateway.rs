use crate::cli::{
    application::{CliError, Gateway, TokenRequest},
    infrastructure::LocalGateway,
};
use serde_json::{json, Value};
use std::{io::Write, net::TcpListener, thread};
use tungstenite::{accept, Message};

#[test]
fn adapter_rejects_mismatched_request_ids_and_cross_principal_issuance() {
    for mode in ["correlation", "cross-principal", "unsafe-expiry"] {
        let root = tempfile::tempdir().unwrap();
        let token = root.path().join("cli.token");
        nessa_local_storage::open(&token, nessa_local_storage::OpenMode::CreateNew)
            .unwrap()
            .write_all(b"private-token")
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = accept(stream).unwrap();
            socket.send(Message::Text(json!({"type":"event","event":"session.challenge","payload":{"minVersion":1,"maxVersion":1,"nonce":"challenge","expiresAt":9999999999_u64}}).to_string().into())).unwrap();
            let auth: Value =
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(auth["params"]["client"]["id"], "nessa-cli");
            assert_eq!(auth["params"]["credential"], "private-token");
            socket.send(Message::Text(json!({"type":"res","id":auth["id"],"ok":true,"payload":{
                "version":1,"gatewayId":"gateway","organizationId":"org","principalId":"caller","membershipId":"member","credentialId":"cli","audienceId":"gateway","expiresAt":null,"grants":[],"methods":[]
            }}).to_string().into())).unwrap();
            let issue: Value =
                serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(issue["params"]["membership"]["role"], "member");
            let mut metadata = json!({"id":"issued","principalId":"browser","organizationId":"org","audienceId":"gateway","issuedAt":100,"expiresAt":200,"revokedAt":null,"grants":issue["params"]["grants"]});
            if mode == "cross-principal" {
                metadata["principalId"] = json!("other");
            }
            if mode == "unsafe-expiry" {
                metadata["expiresAt"] = json!(9_007_199_254_740_992_u64);
            }
            socket.send(Message::Text(json!({"type":"res","id":if mode == "correlation" { json!("wrong-request") } else { issue["id"].clone() },"ok":true,"payload":{"credential":metadata,"secret":"never-print-this"}}).to_string().into())).unwrap();
        });
        let mut gateway = LocalGateway::connect(address, &token).unwrap();
        let error = gateway
            .issue(TokenRequest {
                request_id: "request".into(),
                principal_id: "browser".into(),
                membership_id: "browser-member".into(),
                gateway_id: "gateway".into(),
                organization_id: "org".into(),
                expires_at: Some(200),
            })
            .err()
            .unwrap();
        assert!(!error.to_string().contains("never-print-this"));
        assert_eq!(
            error,
            match mode {
                "correlation" => CliError::Correlation,
                "cross-principal" | "unsafe-expiry" => CliError::ScopeMismatch,
                _ => unreachable!(),
            }
        );
        server.join().unwrap();
    }
}

#[test]
fn adapter_rejects_an_authenticated_identity_with_nonportable_expiry() {
    let root = tempfile::tempdir().unwrap();
    let token = root.path().join("cli.token");
    nessa_local_storage::open(&token, nessa_local_storage::OpenMode::CreateNew)
        .unwrap()
        .write_all(b"private-token")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = accept(stream).unwrap();
        socket.send(Message::Text(json!({"type":"event","event":"session.challenge","payload":{"minVersion":1,"maxVersion":1,"nonce":"challenge","expiresAt":9999999999_u64}}).to_string().into())).unwrap();
        let auth: Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
        socket.send(Message::Text(json!({"type":"res","id":auth["id"],"ok":true,"payload":{
            "version":1,"gatewayId":"gateway","organizationId":"org","principalId":"caller","membershipId":"member","credentialId":"cli","audienceId":"gateway","expiresAt":9_007_199_254_740_992_u64,"grants":[],"methods":[]
        }}).to_string().into())).unwrap();
    });

    assert_eq!(
        LocalGateway::connect(address, &token).err(),
        Some(CliError::Protocol)
    );
    server.join().unwrap();
}
