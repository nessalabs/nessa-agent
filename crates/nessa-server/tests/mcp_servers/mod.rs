//! The gateway's side of one MCP connection per server for each harness
//! session: what stands in for a server, the relay socket's hello ("A
//! stand-in" table, Hello and Opening rows), the `mcp-relay` command, and a
//! harness's traffic reaching a real server through both on a session of its
//! own. A real `nessa mcp-relay` process is driven in `tests/mcp_relay.rs`.
//!
//! ```text
//! domain    -> configuration_digest / relay_arguments / admit
//! relay     -> Relay::serve over an in-memory socket -> McpServers -> server.py
//! command   -> relay() against a scripted gateway
//! end_to_end-> relay() -> Relay::serve -> McpServers -> server.py
//! ```
use super::domain::{
    admit, configuration_digest, relay_arguments, StandInRefusal, RELAY_SUBCOMMAND,
};
use super::infrastructure::{
    read_line, relay, write_line, Answer, Hello, ListedToolUis, Refusal, Relay, RelayFailure,
    HELLO_TIMEOUT, MAX_HELLO_BYTES,
};
use crate::conversation::application::McpToolUis;
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::infrastructure::{
    acp::sessions::StdioMcpServer,
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

fn fixture() -> StdioMcpServer {
    StdioMcpServer {
        name: "fixture".into(),
        command: PathBuf::from("/usr/bin/python3"),
        args: vec![format!(
            "{}/../nessa-sdk/tests/infrastructure/mcp/fixtures/server.py",
            env!("CARGO_MANIFEST_DIR")
        )],
    }
}

fn digest(server: &StdioMcpServer) -> String {
    configuration_digest(&server.command, &server.args)
}

fn relay_for(servers: Vec<StdioMcpServer>) -> (Arc<Relay>, McpServers) {
    let launches = servers
        .iter()
        .map(|server| McpServerLaunch {
            server: server.clone(),
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::new(),
        })
        .collect();
    let mcp = McpServers::new(launches, Arc::new(RuntimeClock::new())).unwrap();
    let digests = servers
        .iter()
        .map(|server| (server.name.clone(), digest(server)))
        .collect();
    (Arc::new(Relay::new(mcp.clone(), digests)), mcp)
}

#[test]
fn the_digest_changes_with_the_command_and_each_argument_and_their_boundaries() {
    let base = fixture();
    let same = configuration_digest(&base.command, &base.args);
    assert_eq!(digest(&base), same);
    assert!(same.starts_with("sha256:") && same.len() == 7 + 64);
    let other_command = configuration_digest(&PathBuf::from("/usr/bin/python"), &base.args);
    let other_args = configuration_digest(&base.command, &["x".into()]);
    // Where one argument ends is part of it: ["ab"] is not ["a", "b"].
    let joined = configuration_digest(&base.command, &["ab".into()]);
    let split = configuration_digest(&base.command, &["a".into(), "b".into()]);
    // And with as many arguments either way: ["ab", "c"] is not ["a", "bc"].
    let left = configuration_digest(&base.command, &["ab".into(), "c".into()]);
    let right = configuration_digest(&base.command, &["a".into(), "bc".into()]);
    let none = configuration_digest(&base.command, &[]);
    let empty = configuration_digest(&base.command, &[String::new()]);
    let all = [
        &same,
        &other_command,
        &other_args,
        &joined,
        &split,
        &left,
        &right,
        &none,
        &empty,
    ];
    for (index, one) in all.iter().enumerate() {
        for other in &all[index + 1..] {
            assert_ne!(one, other);
        }
    }
}

#[test]
fn a_stand_in_runs_the_relay_with_the_socket_name_and_digest() {
    assert_eq!(RELAY_SUBCOMMAND, "mcp-relay");
    assert_eq!(
        relay_arguments("/ns/mcp/relay.sock", "mcptest", "sha256:ab"),
        ["mcp-relay", "/ns/mcp/relay.sock", "mcptest", "sha256:ab"]
    );
}

#[test]
fn a_stand_in_is_let_through_only_for_a_configured_server_as_configured() {
    let configured = BTreeMap::from([("fixture".to_owned(), "sha256:a".to_owned())]);
    assert_eq!(admit("fixture", "sha256:a", &configured), Ok(()));
    assert_eq!(
        admit("fixture", "sha256:b", &configured),
        Err(StandInRefusal::ConfigurationChanged)
    );
    assert_eq!(
        admit("other", "sha256:a", &configured),
        Err(StandInRefusal::UnknownServer)
    );
    // A name is a key, never something an inherited property answers to.
    assert_eq!(
        admit("constructor", "sha256:a", &configured),
        Err(StandInRefusal::UnknownServer)
    );
}

/// The relay's side of one connection, served in the background, and the
/// stand-in's end of it.
fn connect(
    relay: &Arc<Relay>,
) -> (
    BufReader<tokio::io::ReadHalf<DuplexStream>>,
    tokio::io::WriteHalf<DuplexStream>,
) {
    let (stand_in, gateway) = tokio::io::duplex(1024 * 1024);
    let relay = relay.clone();
    tokio::spawn(async move { relay.serve(gateway).await });
    let (read, write) = tokio::io::split(stand_in);
    (BufReader::new(read), write)
}

async fn hello(write: &mut tokio::io::WriteHalf<DuplexStream>, server: &str, configuration: &str) {
    let hello = Hello {
        server: server.into(),
        configuration: configuration.into(),
    };
    write_line(write, &hello).await.unwrap();
}

#[tokio::test]
async fn a_stand_in_for_an_unknown_or_changed_server_is_refused_by_reason() {
    let server = fixture();
    let (relay, _) = relay_for(vec![server.clone()]);
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "other", &digest(&server)).await;
    assert!(matches!(
        read_line::<Answer>(&mut read).await,
        Some(Answer::Refused {
            reason: Refusal::UnknownServer,
            ..
        })
    ));
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "fixture", "sha256:old").await;
    assert!(matches!(
        read_line::<Answer>(&mut read).await,
        Some(Answer::Refused {
            reason: Refusal::ConfigurationChanged,
            ..
        })
    ));
}

#[tokio::test]
async fn a_server_that_cannot_start_refuses_its_stand_in_as_unavailable() {
    let missing = StdioMcpServer {
        name: "missing".into(),
        command: PathBuf::from("/nonexistent/server"),
        args: vec![],
    };
    let (relay, _) = relay_for(vec![missing.clone()]);
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "missing", &digest(&missing)).await;
    let Some(Answer::Refused { reason, message }) = read_line::<Answer>(&mut read).await else {
        panic!("refused");
    };
    assert_eq!(reason, Refusal::Unavailable);
    assert!(message.contains("could not be started"), "{message}");
}

#[tokio::test]
async fn a_hello_that_is_not_one_bounded_json_line_is_closed_without_an_answer() {
    let (relay, _) = relay_for(vec![fixture()]);
    for bytes in [
        b"not json\n".to_vec(),
        b"{\"server\":\"fixture\",\"configuration\":\"x\",\"extra\":1}\n".to_vec(),
        vec![b' '; MAX_HELLO_BYTES + 1],
        // A hello that would be read, but only past the bound.
        [
            vec![b' '; MAX_HELLO_BYTES],
            b"{\"server\":\"fixture\",\"configuration\":\"x\"}\n".to_vec(),
        ]
        .concat(),
    ] {
        let (mut read, mut write) = connect(&relay);
        write.write_all(&bytes).await.unwrap();
        let mut rest = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut read, &mut rest)
            .await
            .unwrap();
        assert!(rest.is_empty(), "{}", String::from_utf8_lossy(&rest));
    }
}

#[tokio::test(start_paused = true)]
async fn a_stand_in_that_never_says_hello_is_closed() {
    let (relay, _) = relay_for(vec![fixture()]);
    let (mut read, _write) = connect(&relay);
    let mut rest = Vec::new();
    let closed = tokio::io::AsyncReadExt::read_to_end(&mut read, &mut rest);
    tokio::time::timeout(HELLO_TIMEOUT * 2, closed)
        .await
        .expect("closed after the hello timeout")
        .unwrap();
    assert!(rest.is_empty());
}

/// A gateway that answers the hello with `answer` and then echoes.
fn scripted(answer: Option<Answer>) -> DuplexStream {
    let (stand_in, gateway) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(gateway);
        let mut read = BufReader::new(read);
        let _: Option<Hello> = read_line(&mut read).await;
        let Some(answer) = answer else {
            std::future::pending::<()>().await;
            return;
        };
        write_line(&mut write, &answer).await.unwrap();
        let mut line = String::new();
        while read.read_line(&mut line).await.unwrap_or(0) > 0 {
            write.write_all(line.as_bytes()).await.unwrap();
            line.clear();
        }
    });
    stand_in
}

#[tokio::test]
async fn the_relay_command_reports_a_refusal() {
    let refused = Answer::Refused {
        reason: Refusal::ConfigurationChanged,
        message: "changed".into(),
    };
    let (input, _keep) = tokio::io::duplex(1024);
    let result = relay(scripted(Some(refused)), "s", "c", input, tokio::io::sink()).await;
    assert_eq!(
        result,
        Err(RelayFailure::Refused {
            reason: Refusal::ConfigurationChanged,
            message: "changed".into()
        })
    );
    assert!(result.unwrap_err().to_string().contains("refused"));
}

#[tokio::test(start_paused = true)]
async fn the_relay_command_gives_up_on_a_gateway_that_never_answers() {
    let (input, _keep) = tokio::io::duplex(1024);
    let result = relay(scripted(None), "s", "c", input, tokio::io::sink()).await;
    assert_eq!(result, Err(RelayFailure::NoAnswer));
    assert!(!RelayFailure::Unreachable.to_string().is_empty());
}

#[tokio::test]
async fn the_relay_command_copies_both_ways_until_the_gateway_closes() {
    let (mut harness_in, input) = tokio::io::duplex(1024);
    let (output, harness_out) = tokio::io::duplex(1024);
    let running = tokio::spawn(relay(
        scripted(Some(Answer::Accepted)),
        "s",
        "c",
        input,
        output,
    ));
    harness_in.write_all(b"{\"a\":1}\n").await.unwrap();
    let mut echoed = BufReader::new(harness_out);
    let mut line = String::new();
    echoed.read_line(&mut line).await.unwrap();
    assert_eq!(line, "{\"a\":1}\n");
    // The harness closes its input; the gateway, seeing that, closes; the
    // stand-in ends.
    drop(harness_in);
    assert_eq!(running.await.unwrap(), Ok(()));
}

/// A harness reaches the real server through `mcp-relay` and the relay
/// socket, on a session of its own, and the view's lookup reads that
/// session's list; the session ends with the stand-in.
#[tokio::test]
async fn a_harness_through_the_relay_gets_a_session_of_its_own() {
    let server = fixture();
    let (relay_side, mcp) = relay_for(vec![server.clone()]);
    let (stand_in, gateway) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(async move { relay_side.serve(gateway).await });
    let (mut harness_in, input) = tokio::io::duplex(64 * 1024);
    let (output, harness_out) = tokio::io::duplex(64 * 1024);
    let configuration = digest(&server);
    let relayed =
        tokio::spawn(
            async move { relay(stand_in, "fixture", &configuration, input, output).await },
        );
    let mut answers = BufReader::new(harness_out).lines();
    let ask = |message: Value| {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        bytes
    };
    harness_in
        .write_all(&ask(
            json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {} }),
        ))
        .await
        .unwrap();
    let initialized: Value =
        serde_json::from_str(&answers.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "fixture-process"
    );
    // Listed first, as a harness lists before it calls.
    harness_in
        .write_all(&ask(
            json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" }),
        ))
        .await
        .unwrap();
    answers.next_line().await.unwrap().unwrap();
    harness_in
        .write_all(&ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "where" } })))
        .await
        .unwrap();
    let place: Value = serde_json::from_str(&answers.next_line().await.unwrap().unwrap()).unwrap();
    let pid = place["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap() as libc::pid_t;
    // The view's lookup reads the session's list, once it has been read.
    let uis = ListedToolUis(mcp.clone());
    let chart = McpTool::new("fixture", "show_chart").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while uis.resource_uri(&chart).is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the session's tools are listed");
    assert_eq!(
        uis.resource_uri(&chart).unwrap().as_str(),
        "ui://fixture/chart.html"
    );
    assert_eq!(
        uis.resource_uri(&McpTool::new("fixture", "remember").unwrap()),
        None
    );
    // The harness goes; its stand-in and session end, and the server with them.
    drop(harness_in);
    assert_eq!(relayed.await.unwrap(), Ok(()));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the server ends with its stand-in");
    assert_eq!(uis.resource_uri(&chart), None);
}

#[test]
fn a_refusals_message_is_one_bounded_line() {
    use super::infrastructure::said;
    assert_eq!(said("could not\nstart\u{7}"), "could not start ");
    let long = "é".repeat(600);
    assert_eq!(said(&long).chars().count(), 512);
    // However JSON escapes what is left, the answer stays within the bound.
    let answer = Answer::Refused {
        reason: Refusal::Unavailable,
        message: said(&"\\".repeat(600)),
    };
    assert!(serde_json::to_vec(&answer).unwrap().len() < MAX_HELLO_BYTES);
}

#[tokio::test]
async fn a_relay_socket_a_gateway_holds_is_not_taken_over() {
    use super::infrastructure::bind;
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("relay").join("relay.sock");
    let serving = bind(&socket).await.unwrap();
    let refused = bind(&socket).await.unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::AddrInUse);
    // The lock decides, not the socket: with its file gone, still refused,
    // and nothing is made in its place.
    std::fs::remove_file(&socket).unwrap();
    assert_eq!(
        bind(&socket).await.unwrap_err().kind(),
        std::io::ErrorKind::AddrInUse
    );
    assert!(std::fs::symlink_metadata(&socket).is_err());
    // Once released, it is bound again. A child another test forks at that
    // moment holds the lock until it execs, so allow it an instant.
    drop(serving);
    let started = std::time::Instant::now();
    while let Err(error) = bind(&socket).await {
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "still refused: {error:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn the_relay_lock_is_the_sockets_name_private_and_never_followed() {
    use super::infrastructure::bind;
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("relay").join("relay.sock");
    let bound = bind(&socket).await.unwrap();
    let lock = std::fs::symlink_metadata(socket.parent().unwrap().join("relay.sock.lock")).unwrap();
    assert!(lock.is_file());
    assert_eq!(lock.permissions().mode() & 0o777, 0o600);
    drop(bound);
    // A link where the lock goes is refused, not followed: nothing is bound.
    let other = directory.path().join("relay").join("other.sock");
    let target = directory.path().join("elsewhere");
    std::os::unix::fs::symlink(&target, socket.parent().unwrap().join("other.sock.lock")).unwrap();
    assert!(bind(&other).await.is_err());
    assert!(std::fs::symlink_metadata(&target).is_err());
    assert!(std::fs::symlink_metadata(&other).is_err());
}

#[tokio::test]
async fn a_socket_an_earlier_run_left_is_replaced() {
    use super::infrastructure::bind;
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("relay").join("relay.sock");
    nessa_local_storage::create_directory(socket.parent().unwrap()).unwrap();
    // A socket file with no gateway, and no lock held: what a crash leaves.
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    assert!(std::fs::symlink_metadata(&socket).is_ok());
    let bound = bind(&socket).await.unwrap();
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_ok());
    drop(bound);
}
