//! A real `nessa mcp-relay` process against a relay socket served here, with
//! a real server behind it ("A session" and "A stand-in" tables): a relay
//! killed outright still ends its server and everything the server started,
//! a relay whose server ends exits at once though its stdin stays open, and
//! a relay reads its session token from its environment, never its arguments.
#![cfg(unix)]

use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::{
    acp::sessions::{StandInGrant, StandInGrants, StdioMcpServer},
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers, MCP_SESSION_VARIABLE},
};
use nessa_server::mcp_servers::{
    domain::{configuration_digest, ConfigurationKey},
    infrastructure::{bind, ConversationGrants, OsTokens, Relay},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(args: &[&str]) -> StdioMcpServer {
    let mut arguments = vec![format!(
        "{}/../nessa-sdk/tests/infrastructure/mcp/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    )];
    arguments.extend(args.iter().map(|arg| (*arg).to_owned()));
    StdioMcpServer {
        name: "fixture".into(),
        command: PathBuf::from("/usr/bin/python3"),
        args: arguments,
    }
}

/// The key the relay served here digests with.
fn key() -> ConfigurationKey {
    ConfigurationKey::new([7; 32])
}

/// A relay socket in `directory` serving `server`, until the runtime ends,
/// and a conversation's grant with the token its stand-ins carry.
async fn serve(directory: &Path, server: &StdioMcpServer) -> (PathBuf, StandInGrant, String) {
    // In a directory of its own, created private by `bind`, as the gateway's is.
    let socket = directory.join("relay").join("relay.sock");
    let servers = McpServers::new(
        vec![McpServerLaunch {
            server: server.clone(),
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::new(),
        }],
        Arc::new(RuntimeClock::new()),
    )
    .unwrap();
    let listener = bind(&socket).await.unwrap();
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    let grant = grants.grant(&SessionId::new("conversation").unwrap());
    let token = grant.environment()[0].1.clone();
    tokio::spawn(Arc::new(Relay::new(servers, grants, key())).listen(listener));
    (socket, grant, token)
}

struct RelayProcess {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl RelayProcess {
    /// The relay for `server`, its session token in its environment when
    /// there is one, as a harness gives it.
    fn start(socket: &Path, server: &StdioMcpServer, token: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nessa"));
        command.env_remove(MCP_SESSION_VARIABLE);
        if let Some(token) = token {
            command.env(MCP_SESSION_VARIABLE, token);
        }
        let mut child = command
            .args([
                "mcp-relay",
                socket.to_str().unwrap(),
                &server.name,
                &configuration_digest(&key(), &server.command, &server.args, &BTreeMap::new()),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }
    /// List the tools, as a harness does before it calls.
    async fn listed(self) -> Self {
        self.ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" }))
            .await
            .0
    }
    /// Ask `name` of the server and read the answer, off the runtime.
    async fn call(self, id: u64, name: &str) -> (Self, Value) {
        self.ask(json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name } }))
            .await
    }
    async fn ask(mut self, request: Value) -> (Self, Value) {
        tokio::task::spawn_blocking(move || {
            writeln!(self.input, "{request}").unwrap();
            let mut line = String::new();
            self.output.read_line(&mut line).unwrap();
            let answer = serde_json::from_str(&line).unwrap();
            (self, answer)
        })
        .await
        .unwrap()
    }
}

/// Whether `pid` is still running: a zombie is not. A descendant killed with
/// its group is reaped by whatever adopted it, which in a container may be
/// never.
fn alive(pid: i64) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    let state = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    !state.trim().is_empty() && !state.trim_start().starts_with('Z')
}

async fn gone(pid: i64) {
    let started = Instant::now();
    while alive(pid) {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "process {pid} is still running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_killed_outright_ends_its_server_and_everything_it_started() {
    let directory = tempfile::tempdir().unwrap();
    let server = fixture(&["--child"]);
    let (socket, _grant, token) = serve(directory.path(), &server).await;
    let relay = RelayProcess::start(&socket, &server, Some(&token))
        .listed()
        .await;
    let (mut relay, place) = relay.call(1, "where").await;
    let pid = place["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap();
    let child = place["result"]["structuredContent"]["child"]
        .as_i64()
        .unwrap();
    assert!(alive(pid) && alive(child));
    // SIGKILL: no clean close of anything.
    relay.child.kill().unwrap();
    relay.child.wait().unwrap();
    gone(pid).await;
    gone(child).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_whose_server_ends_exits_though_its_stdin_stays_open() {
    let directory = tempfile::tempdir().unwrap();
    let server = fixture(&[]);
    let (socket, _grant, token) = serve(directory.path(), &server).await;
    let relay = RelayProcess::start(&socket, &server, Some(&token))
        .listed()
        .await;
    let (relay, _) = relay.call(1, "where").await;
    let RelayProcess {
        mut child,
        mut input,
        output,
    } = relay;
    // The server exits mid-call; the harness's stdin stays open throughout.
    writeln!(
        input,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "exit" } })
    )
    .unwrap();
    let started = Instant::now();
    let status = tokio::task::spawn_blocking(move || loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the relay is still running"
        );
        std::thread::sleep(Duration::from_millis(20));
    })
    .await
    .unwrap();
    assert!(status.success(), "{status}");
    drop((input, output));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relay_refused_exits_with_a_failure() {
    let directory = tempfile::tempdir().unwrap();
    let server = fixture(&[]);
    let (socket, _grant, token) = serve(directory.path(), &server).await;
    let stale = StdioMcpServer {
        args: vec!["/old.py".into()],
        ..server.clone()
    };
    // Refused for a changed server, and for a token never given or never
    // issued.
    for (configured, token) in [
        (&stale, Some(token.as_str())),
        (&server, None),
        (&server, Some("forged")),
    ] {
        let mut relay = RelayProcess::start(&socket, configured, token);
        let status = tokio::task::spawn_blocking(move || relay.child.wait().unwrap())
            .await
            .unwrap();
        assert!(!status.success());
    }
}
