//! The real retained CLI child against a disposable numeric-loopback peer.
//! No Nessa data store is opened and every credential/phrase is synthetic.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::{accept, Message};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run(reply: &'static str) {
    let root = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let peer = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = accept(stream).unwrap();
        socket.send(Message::Text(json!({"type":"event","event":"session.challenge","payload":{"minVersion":1,"maxVersion":1,"nonce":"synthetic-challenge","expiresAt":1},"seq":0,"stateVersion":0}).to_string().into())).unwrap();
        let auth = socket.read().unwrap();
        let auth: Value = serde_json::from_str(auth.to_text().unwrap()).unwrap();
        assert_eq!(auth["method"], "session.authenticate");
        assert_eq!(auth["params"]["credential"], "synthetic-bridge-credential");
        socket.send(Message::Text(json!({"type":"res","id":auth["id"],"ok":true,"payload":{"version":1,"gatewayId":"synthetic-gateway","principalId":"synthetic-owner","organizationId":"synthetic-org","membershipId":"synthetic-member","credentialId":"synthetic-credential","audienceId":"synthetic-audience","expiresAt":null,"grants":[],"methods":["vault.changePassphrase"]}}).to_string().into())).unwrap();
        let request = socket.read().unwrap();
        let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
        observed.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request["method"], "vault.changePassphrase");
        assert_eq!(
            request["params"],
            json!({"passphrase":"disposable replacement bridge phrase"})
        );
        if reply == "lost" {
            return;
        }
        let response = match reply {
            "committed" => {
                json!({"type":"res","id":request["id"],"ok":true,"payload":{"state":"locked"}})
                    .to_string()
            }
            "correlation" => {
                json!({"type":"res","id":"departed-request","ok":true,"payload":{"state":"locked"}})
                    .to_string()
            }
            "malformed" => "{not-json}".to_owned(),
            "payload-null" => json!({"type":"res","id":request["id"],"ok":true,"payload":null}).to_string(),
            "payload-unlocked" => json!({"type":"res","id":request["id"],"ok":true,"payload":{"state":"unlocked"}}).to_string(),
            "missing-ok" => json!({"type":"res","id":request["id"],"error":{"code":"vault_change_unchanged","message":"redacted synthetic refusal"}}).to_string(),
            code => json!({"type":"res","id":request["id"],"ok":false,"error":{"code":code,"message":"redacted synthetic refusal"}})
                .to_string(),
        };
        socket.send(Message::Text(response.into())).unwrap();
        match socket.read() {
            Err(_) | Ok(Message::Close(_)) => (),
            Ok(_) => panic!("retained child must not replay a change"),
        }
    });
    let child = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args(["vault", "session"])
        .env("NESSA_STAGE", "ci")
        .env("NESSA_DATA_DIR", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = OwnedChild(child);
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(4);
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let value = line
                .ok()
                .and_then(|line| serde_json::from_str::<Value>(&line).ok());
            if send.send(value).is_err() {
                return;
            }
        }
    });
    writeln!(
        input,
        "{}",
        json!({"address":address.to_string(),"credential":"synthetic-bridge-credential"})
    )
    .unwrap();
    input.flush().unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(10)).unwrap(),
        Some(json!({"ready":true}))
    );
    writeln!(
        input,
        "{}",
        json!({"operation":"changePassphrase","passphrase":"disposable replacement bridge phrase"})
    )
    .unwrap();
    input.flush().unwrap();
    let result = receive
        .recv_timeout(Duration::from_secs(10))
        .expect("child must provide a bounded operation-aware answer")
        .unwrap();
    let expected = match reply {
        "committed" => json!({"ok":true,"status":{"state":"locked"}}),
        "forbidden" | "unauthorized" => json!({"ok":false,"code":"vault_forbidden"}),
        "lost"
        | "correlation"
        | "malformed"
        | "unknown-server-code"
        | "payload-null"
        | "payload-unlocked"
        | "missing-ok" => {
            json!({"ok":false,"code":"vault_change_not_confirmed"})
        }
        code => json!({"ok":false,"code":code}),
    };
    assert_eq!(result, expected);
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "child must end after input closes"
        );
        thread::sleep(Duration::from_millis(1));
    }
    reader.join().unwrap();
    peer.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn retained_cli_routes_change_and_preserves_publication_audit_outcomes() {
    for reply in [
        "committed",
        "vault_change_unchanged",
        "vault_change_unchanged_audit_unavailable",
        "vault_change_not_confirmed",
        "vault_change_not_confirmed_audit_unavailable",
        "vault_changed_audit_unavailable",
        "vault_busy",
        "forbidden",
        "unauthorized",
    ] {
        run(reply);
    }
}
#[test]
fn retained_cli_lost_answers_are_uncertain_without_replay() {
    for reply in [
        "lost",
        "correlation",
        "malformed",
        "unknown-server-code",
        "payload-null",
        "payload-unlocked",
        "missing-ok",
    ] {
        run(reply);
    }
}
