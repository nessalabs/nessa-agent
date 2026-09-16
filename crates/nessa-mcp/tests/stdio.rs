//! Real MCP client -> process ownership -> private audit evidence.
#![cfg(unix)]
use serde_json::{json, Value};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Client {
    async fn new(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nessa-mcp"))
            .args([
                "--workspace",
                root.to_str().unwrap(),
                "--audit-directory",
                root.join("audit").to_str().unwrap(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut client = Self {
            child,
            input,
            output,
        };
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
        assert_eq!(
            client.read().await["result"]["serverInfo"]["name"],
            "nessa-mcp"
        );
        client
    }
    async fn send(&mut self, value: Value) {
        self.raw(&(value.to_string() + "\n")).await;
    }
    async fn raw(&mut self, value: &str) {
        self.input
            .as_mut()
            .unwrap()
            .write_all(value.as_bytes())
            .await
            .unwrap();
    }
    async fn read(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), self.output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    async fn close(mut self) {
        self.input.take();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
    async fn shell(&mut self, command: &str, timeout: u64) {
        self.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shell","arguments":{"command":command,"timeoutSeconds":timeout},"_meta":{"actor":"forged-user","requestId":"forged-request"}}})).await;
    }
}
fn records(root: &Path) -> Vec<Value> {
    std::fs::read_dir(root.join("audit"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|s| s.to_str()) == Some("json"))
                .then(|| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        })
        .collect()
}
async fn wait_started(root: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if records(root).iter().any(|r| r["event"] == "started") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn result(reply: &Value) -> Value {
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn assert_provider_correlation(records: &[Value]) {
    let connection = records[0]["correlation"]["connectionId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("Nessa-generated connection identity");
    assert!(records.iter().all(|record| {
        record["correlation"]["connectionId"] == connection
            && record["correlation"]["requestId"] == 2
            && record["correlation"]["initiator"] == "configured_provider_tool_call"
    }));
    let admitted = records
        .iter()
        .find(|record| record["event"] == "admitted")
        .expect("admission audit record");
    assert_eq!(
        admitted["state"]["initiator"],
        admitted["correlation"]["initiator"]
    );
    assert!(records.iter().all(|record| {
        record["correlation"]["connectionId"] != "forged-user"
            && record["correlation"]["requestId"] != "forged-request"
    }));
}

#[tokio::test]
async fn command_result_and_audit_keep_identity_output_and_cleanup_together() {
    let root = tempfile::tempdir().unwrap();
    let mut client = Client::new(root.path()).await;
    client
        .shell("printf 'hello'; printf 'problem' >&2; exit 7", 10)
        .await;
    let reply = client.read().await;
    assert_eq!(reply["result"]["isError"], true);
    let output = result(&reply);
    assert_eq!(output["stdout"], "hello");
    assert_eq!(output["stderr"], "problem");
    assert_eq!(output["exitCode"], 7);
    assert_eq!(output["cleanupVerified"], true);
    client.close().await;
    let records = records(root.path());
    assert_eq!(records.len(), 3);
    assert_provider_correlation(&records);
    assert!(records
        .iter()
        .all(|r| r["commandId"] == output["commandId"]));
    let start = records.iter().find(|r| r["event"] == "started").unwrap();
    let end = records.iter().find(|r| r["event"] == "finished").unwrap();
    assert_eq!(start["state"]["osPid"], output["osPid"]);
    assert_eq!(end["state"]["exitCode"], 7);
    assert_eq!(end["state"]["cleanupVerified"], true);
}
#[tokio::test]
async fn cancellation_and_client_loss_finish_owned_work_and_record_the_real_cause() {
    for cancel in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let mut client = Client::new(root.path()).await;
        client.shell("sleep 30", 60).await;
        wait_started(root.path()).await;
        if cancel {
            client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}})).await;
            assert_eq!(
                result(&client.read().await)["cause"],
                "Stopped(ClientCancelled)"
            );
        }
        client.close().await;
        let records = records(root.path());
        assert_eq!(records.len(), 3);
        assert_provider_correlation(&records);
        let end = records.iter().find(|r| r["event"] == "finished").unwrap();
        assert_eq!(end["state"]["cleanupVerified"], true);
        assert_eq!(
            end["state"]["cause"],
            if cancel {
                "Stopped(ClientCancelled)"
            } else {
                "Stopped(ClientClosed)"
            }
        );
    }
}
#[tokio::test]
async fn malformed_arguments_never_spawn_and_output_overflow_is_explicit() {
    let root = tempfile::tempdir().unwrap();
    let mut client = Client::new(root.path()).await;
    client.raw("{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"shell\",\"arguments\":{\"command\":\"true\",\"command\":\"false\"}}}\n").await;
    assert_eq!(client.read().await["error"]["code"], -32602);
    assert!(records(root.path()).is_empty());
    client
        .shell("head -c 300000 /dev/zero | tr '\\0' x", 10)
        .await;
    let output = result(&client.read().await);
    assert!(output["stdout"].as_str().unwrap().len() <= 65536);
    assert!(output["droppedBytes"].as_u64().unwrap() > 0);
    assert_eq!(output["cleanupVerified"], true);
    client.close().await;
}
#[tokio::test]
async fn timeout_is_distinct_from_process_exit() {
    let root = tempfile::tempdir().unwrap();
    let mut client = Client::new(root.path()).await;
    client.shell("sleep 30", 1).await;
    let output = result(&client.read().await);
    assert_eq!(output["cause"], "Timeout");
    assert_eq!(output["cleanupVerified"], true);
    client.close().await;
}
