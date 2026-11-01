use super::{after_dispatch, LocalProduct, LocalProductError, LocalProductIdentity};
use nessa_protocol::product::generated::VaultStatus;
use serde_json::{json, Value};
use std::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tungstenite::{protocol::Role, Message, WebSocket};

fn peer(
    run: impl FnOnce(WebSocket<TcpStream>, Value) + Send + 'static,
) -> (LocalProduct, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut socket = WebSocket::from_raw_socket(stream, Role::Server, None);
        let request = socket.read().unwrap();
        let request = serde_json::from_str(request.to_text().unwrap()).unwrap();
        run(socket, request);
    });
    (
        LocalProduct {
            socket: WebSocket::from_raw_socket(stream, Role::Client, None),
            identity: LocalProductIdentity {
                gateway_id: "synthetic-gateway".into(),
                organization_id: "synthetic-org".into(),
                principal_id: "synthetic-owner".into(),
                expires_at: None,
            },
            deadline: Instant::now() + Duration::from_secs(2),
            next_id: 0,
        },
        worker,
    )
}
fn send(socket: &mut WebSocket<TcpStream>, response: Value) {
    socket
        .send(Message::Text(response.to_string().into()))
        .unwrap();
}
fn assert_uncertain(error: LocalProductError) {
    assert!(
        matches!(error, LocalProductError::Vault(code) if code.as_str() == "vault_change_not_confirmed"),
        "{error:?}"
    );
}

#[test]
fn lost_correlated_and_invalid_change_answers_preserve_uncertainty() {
    for bad in ["closed", "correlation", "payload", "unknown-error"] {
        let (mut client, worker) = peer(move |mut socket, request| {
            assert_eq!(request["method"], "vault.changePassphrase");
            if bad == "closed" {
                return;
            }
            let response = match bad {
                "correlation" => {
                    json!({"type":"res","id":"another-request","ok":true,"payload":{"state":"locked"}})
                }
                "payload" => {
                    json!({"type":"res","id":request["id"],"ok":true,"payload":{"state":"unrecognized"}})
                }
                _ => {
                    json!({"type":"res","id":request["id"],"ok":false,"error":{"code":"unknown-provider-text","message":"redacted synthetic refusal"}})
                }
            };
            send(&mut socket, response);
            assert!(socket.read().is_err(), "change must not be replayed");
        });
        let result: Result<VaultStatus, _> = client.request(
            "vault.changePassphrase",
            json!({"passphrase":"disposable replacement phrase"}),
        );
        assert_uncertain(result.err().unwrap());
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn recognized_change_publication_audit_and_preseal_refusals_are_preserved() {
    for code in [
        "vault_change_unchanged",
        "vault_change_unchanged_audit_unavailable",
        "vault_change_not_confirmed",
        "vault_change_not_confirmed_audit_unavailable",
        "vault_changed_audit_unavailable",
        "vault_busy",
        "vault_locked",
        "vault_session_owned",
        "vault_passphrase_policy",
        "vault_forbidden",
    ] {
        let (mut client, worker) = peer(move |mut socket, request| {
            send(
                &mut socket,
                json!({"type":"res","id":request["id"],"ok":false,"error":{"code":code,"message":"redacted synthetic refusal"}}),
            );
        });
        let result: Result<Value, _> = client.request(
            "vault.changePassphrase",
            json!({"passphrase":"disposable replacement phrase"}),
        );
        assert!(matches!(result, Err(LocalProductError::Vault(actual)) if actual.as_str() == code));
        worker.join().unwrap();
    }
}

#[test]
fn cancellation_after_dispatch_is_an_uncertain_change() {
    let (seen, receive) = mpsc::sync_channel(1);
    let (mut client, server) = peer(move |mut socket, request| {
        assert_eq!(request["method"], "vault.changePassphrase");
        seen.send(()).unwrap();
        assert!(socket.read().is_err());
    });
    let cancellation = client.cancellation().unwrap();
    let caller = thread::spawn(move || {
        client.request::<Value>(
            "vault.changePassphrase",
            json!({"passphrase":"disposable replacement phrase"}),
        )
    });
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    cancellation.close();
    assert_uncertain(caller.join().unwrap().err().unwrap());
    server.join().unwrap();
}

#[test]
fn non_mutation_transport_errors_keep_their_existing_meaning() {
    for error in [
        LocalProductError::Transport,
        LocalProductError::TimedOut,
        LocalProductError::Protocol,
        LocalProductError::Correlation,
        LocalProductError::Rejected,
    ] {
        assert_eq!(
            after_dispatch::<()>("vault.status", Err(error), || {}),
            Err(error)
        );
        assert_uncertain(
            after_dispatch::<()>("vault.changePassphrase", Err(error), || {})
                .err()
                .unwrap(),
        );
    }
    for error in [LocalProductError::Unauthorized, LocalProductError::Denied] {
        assert_eq!(
            after_dispatch::<()>("vault.changePassphrase", Err(error), || {}),
            Err(error)
        );
    }
}

#[test]
fn an_unconfirmed_change_retires_transport_before_the_client_is_dropped() {
    let (ended, observed) = mpsc::sync_channel(1);
    let (mut client, worker) = peer(move |mut socket, _| {
        send(
            &mut socket,
            json!({"type":"res","id":"wrong-reply","ok":true,"payload":{"state":"locked"}}),
        );
        let error = socket
            .read()
            .expect_err("no next request may use the uncertain connection");
        let timed_out = matches!(error, tungstenite::Error::Io(ref cause) if matches!(cause.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
        ended.send(!timed_out).unwrap();
    });
    assert_uncertain(
        client
            .request::<Value>(
                "vault.changePassphrase",
                json!({"passphrase":"disposable replacement phrase"}),
            )
            .err()
            .unwrap(),
    );
    assert!(
        observed.recv_timeout(Duration::from_secs(3)).unwrap(),
        "the transport must close while the caller still owns the client"
    );
    drop(client);
    worker.join().unwrap();
}

#[test]
fn a_recognized_preseal_refusal_keeps_the_existing_owner_connection() {
    let (mut client, worker) = peer(move |mut socket, first| {
        send(
            &mut socket,
            json!({"type":"res","id":first["id"],"ok":false,"error":{"code":"vault_busy","message":"redacted synthetic refusal"}}),
        );
        let second = socket.read().unwrap();
        let second: Value = serde_json::from_str(second.to_text().unwrap()).unwrap();
        assert_eq!(second["method"], "vault.status");
        send(
            &mut socket,
            json!({"type":"res","id":second["id"],"ok":true,"payload":{"state":"unlocked"}}),
        );
    });
    let changed: Result<Value, _> = client.request(
        "vault.changePassphrase",
        json!({"passphrase":"disposable replacement phrase"}),
    );
    assert!(
        matches!(changed, Err(LocalProductError::Vault(code)) if code.as_str() == "vault_busy")
    );
    let state: Value = client.request("vault.status", json!({})).unwrap();
    assert_eq!(state["state"], "unlocked");
    worker.join().unwrap();
}

#[test]
fn value_callers_cannot_trust_malformed_change_envelopes_or_success_payloads() {
    for bad in [
        "payload-null",
        "payload-unknown",
        "payload-unlocked",
        "missing-ok",
        "string-ok",
        "contradictory",
        "extra",
        "duplicate-ok",
        "duplicate-state",
    ] {
        let (ended, observed) = mpsc::sync_channel(1);
        let (mut client, worker) = peer(move |mut socket, request| {
            let id = &request["id"];
            let response = match bad {
                "payload-null" => json!({"type":"res","id":id,"ok":true,"payload":null}).to_string(),
                "payload-unknown" => json!({"type":"res","id":id,"ok":true,"payload":{"state":"unrecognized"}}).to_string(),
                "payload-unlocked" => json!({"type":"res","id":id,"ok":true,"payload":{"state":"unlocked"}}).to_string(),
                "missing-ok" => json!({"type":"res","id":id,"error":{"code":"vault_change_unchanged","message":"redacted synthetic refusal"}}).to_string(),
                "string-ok" => json!({"type":"res","id":id,"ok":"false","error":{"code":"vault_change_unchanged","message":"redacted synthetic refusal"}}).to_string(),
                "contradictory" => json!({"type":"res","id":id,"ok":true,"payload":{"state":"locked"},"error":{"code":"vault_change_unchanged","message":"redacted synthetic refusal"}}).to_string(),
                "extra" => json!({"type":"res","id":id,"ok":true,"payload":{"state":"locked"},"extra":true}).to_string(),
                "duplicate-ok" => format!("{{\"type\":\"res\",\"id\":{id},\"ok\":true,\"ok\":true,\"payload\":{{\"state\":\"locked\"}}}}"),
                _ => format!("{{\"type\":\"res\",\"id\":{id},\"ok\":true,\"payload\":{{\"state\":\"unlocked\",\"state\":\"locked\"}}}}"),
            };
            socket.send(Message::Text(response.into())).unwrap();
            // Even a later valid reply must not keep the old connection alive.
            let _ = socket.send(Message::Text(
                json!({"type":"res","id":id,"ok":true,"payload":{"state":"locked"}})
                    .to_string()
                    .into(),
            ));
            let error = socket
                .read()
                .expect_err("no next request on uncertain transport");
            let timed_out = matches!(error, tungstenite::Error::Io(ref cause) if matches!(cause.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut));
            ended.send(!timed_out).unwrap();
        });
        assert_uncertain(
            client
                .request::<Value>(
                    "vault.changePassphrase",
                    json!({"passphrase":"disposable replacement phrase"}),
                )
                .err()
                .unwrap(),
        );
        assert!(
            observed.recv_timeout(Duration::from_secs(3)).unwrap(),
            "invalid answer must retire transport while client is retained"
        );
        assert!(
            client.request::<Value>("vault.status", json!({})).is_err(),
            "late reply must not satisfy a new request"
        );
        drop(client);
        worker.join().unwrap();
    }
}

#[test]
fn bounded_valid_notices_preserve_correlation_and_change_uncertainty() {
    for method in ["vault.status", "vault.changePassphrase"] {
        let (mut client, worker) = peer(|mut socket, _| {
            for sequence in 1..=65 {
                send(
                    &mut socket,
                    json!({"type":"event","event":"permission.review.waiting","payload":{},"seq":sequence,"stateVersion":0}),
                );
            }
        });
        let params = if method == "vault.changePassphrase" {
            json!({"passphrase":"generated synthetic replacement phrase"})
        } else {
            json!({})
        };
        let error = client.request::<Value>(method, params).unwrap_err();
        if method == "vault.changePassphrase" {
            assert_uncertain(error);
        } else {
            assert_eq!(error, LocalProductError::Correlation);
        }
        drop(client);
        worker.join().unwrap();
    }
}
