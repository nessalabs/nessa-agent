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
    admit, configuration_digest, relay_arguments, ConfigurationKey, StandInRefusal,
    RELAY_SUBCOMMAND,
};
use super::infrastructure::{
    read_line, relay, write_line, Answer, ConversationGrants, Hello, ListedToolUis, OsTokens,
    Refusal, Relay, RelayFailure, TokenSource, HELLO_TIMEOUT, MAX_HELLO_BYTES,
};
use nessa_protocol::conversation::{domain::ConversationId, tool_uis::McpToolUis};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::infrastructure::{
    acp::sessions::{StandInGrant, StandInGrants, StdioMcpServer},
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers, MCP_SESSION_VARIABLE},
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, sync::Arc};
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

/// The key these tests' relays digest with.
fn key() -> ConfigurationKey {
    ConfigurationKey::new([7; 32])
}

/// The digest of `server` started with no environment, under [`key`]: what
/// [`relay_for`]'s stand-ins carry.
fn digest(server: &StdioMcpServer) -> String {
    configuration_digest(&key(), &server.command, &server.args, &BTreeMap::new())
}

/// The relay for `servers`, the sessions behind it, and the grants it lets
/// stand-ins through by.
fn relay_for(servers: Vec<StdioMcpServer>) -> (Arc<Relay>, McpServers, ConversationGrants) {
    let launches = servers
        .iter()
        .map(|server| McpServerLaunch {
            server: server.clone(),
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::new(),
        })
        .collect();
    let mcp = McpServers::new(launches, Arc::new(RuntimeClock::new())).unwrap();
    let grants = ConversationGrants::new(mcp.clone(), Arc::new(OsTokens));
    (
        Arc::new(Relay::new(mcp.clone(), grants.clone(), key())),
        mcp,
        grants,
    )
}

/// A grant for `conversation`'s open, and the token its stand-ins carry.
fn granted(grants: &ConversationGrants, conversation: &str) -> (StandInGrant, String) {
    let grant = grants.grant(&SessionId::new(conversation).unwrap());
    let token = grant
        .environment()
        .iter()
        .find(|(name, _)| name == MCP_SESSION_VARIABLE)
        .map(|(_, token)| token.clone())
        .expect("a token");
    (grant, token)
}

#[test]
fn the_digest_changes_with_the_command_each_argument_each_variable_and_their_boundaries() {
    let base = fixture();
    let none = BTreeMap::new();
    let with = |environment: &[(&str, &str)]| -> BTreeMap<OsString, OsString> {
        environment
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect()
    };
    let at = |args: &[String], environment: &BTreeMap<OsString, OsString>| {
        configuration_digest(&key(), &base.command, args, environment)
    };
    let same = at(&base.args, &none);
    assert_eq!(digest(&base), same);
    assert!(same.starts_with("hmac-sha256:") && same.len() == 12 + 64);
    let other_command =
        configuration_digest(&key(), &PathBuf::from("/usr/bin/python"), &base.args, &none);
    let other_args = at(&["x".into()], &none);
    // Where one argument ends is part of it: ["ab"] is not ["a", "b"].
    let joined = at(&["ab".into()], &none);
    let split = at(&["a".into(), "b".into()], &none);
    // And with as many arguments either way: ["ab", "c"] is not ["a", "bc"].
    let left = at(&["ab".into(), "c".into()], &none);
    let right = at(&["a".into(), "bc".into()], &none);
    let no_args = at(&[], &none);
    let empty = at(&[String::new()], &none);
    // A variable's value and its name are each part of it, and so is where
    // the one ends and the other begins (#391 PR 2 pass 2b, decision 2).
    let token = at(&base.args, &with(&[("TOKEN", "one")]));
    let rotated = at(&base.args, &with(&[("TOKEN", "two")]));
    let renamed = at(&base.args, &with(&[("TOKEN2", "one")]));
    let shifted = at(&base.args, &with(&[("TOKEN", "2one")]));
    let unset = at(&base.args, &with(&[("TOKEN", "")]));
    let all = [
        &same,
        &other_command,
        &other_args,
        &joined,
        &split,
        &left,
        &right,
        &no_args,
        &empty,
        &token,
        &rotated,
        &renamed,
        &shifted,
        &unset,
    ];
    for (index, one) in all.iter().enumerate() {
        for other in &all[index + 1..] {
            assert_ne!(one, other);
        }
    }
    // Keyed: another process's key gives another digest of the same server.
    let other_key = configuration_digest(
        &ConfigurationKey::new([8; 32]),
        &base.command,
        &base.args,
        &none,
    );
    assert_ne!(other_key, same);
    assert_eq!(format!("{:?}", key()), "ConfigurationKey(..)");
}

/// #391 PR 2 pass 2b, decision 2: a stand-in's arguments, which a process
/// list shows, hold neither a variable's value nor an unkeyed hash of it, nor
/// the unkeyed digest of the configuration — nothing a guessed value could be
/// checked against without the process's key.
#[test]
fn a_stand_ins_arguments_reveal_nothing_about_a_variables_value() {
    use sha2::{Digest, Sha256};
    let base = fixture();
    let value = "secret-value-0123456789";
    let environment = BTreeMap::from([(OsString::from("API_TOKEN"), OsString::from(value))]);
    let digest = configuration_digest(&key(), &base.command, &base.args, &environment);
    let arguments = relay_arguments("/tmp/nessa-mcp-501/s.sock", &base.name, &digest).join(" ");
    let hex = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    // The same fields, length-prefixed as the keyed digest takes them, unkeyed.
    let mut unkeyed = Sha256::new();
    for field in [
        base.command.as_os_str().as_encoded_bytes(),
        &(base.args.len() as u64).to_be_bytes(),
        base.args[0].as_bytes(),
        &1u64.to_be_bytes(),
        b"API_TOKEN",
        value.as_bytes(),
    ] {
        unkeyed.update((field.len() as u64).to_be_bytes());
        unkeyed.update(field);
    }
    let unkeyed = format!("{:x}", unkeyed.finalize());
    for revealing in [
        value.to_owned(),
        hex(value.as_bytes()),
        hex(format!("API_TOKEN={value}").as_bytes()),
        unkeyed,
    ] {
        assert!(!arguments.contains(&revealing), "{arguments}");
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

async fn hello(
    write: &mut tokio::io::WriteHalf<DuplexStream>,
    server: &str,
    configuration: &str,
    session: &str,
) {
    let hello = Hello {
        server: server.into(),
        configuration: configuration.into(),
        session: session.into(),
    };
    write_line(write, &hello).await.unwrap();
}

#[tokio::test]
async fn a_stand_in_for_an_unknown_or_changed_server_is_refused_by_reason() {
    let server = fixture();
    let (relay, _, grants) = relay_for(vec![server.clone()]);
    let (_grant, token) = granted(&grants, "conversation");
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "other", &digest(&server), &token).await;
    assert!(matches!(
        read_line::<Answer>(&mut read).await,
        Some(Answer::Refused {
            reason: Refusal::UnknownServer,
            ..
        })
    ));
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "fixture", "sha256:old", &token).await;
    assert!(matches!(
        read_line::<Answer>(&mut read).await,
        Some(Answer::Refused {
            reason: Refusal::ConfigurationChanged,
            ..
        })
    ));
}

/// What a hello naming `token` is answered, for the configured `server`.
async fn answered(relay: &Arc<Relay>, server: &StdioMcpServer, token: &str) -> Option<Answer> {
    let (mut read, mut write) = connect(relay);
    hello(&mut write, &server.name, &digest(server), token).await;
    read_line::<Answer>(&mut read).await
}

fn unknown_session(answer: Option<Answer>) -> bool {
    matches!(
        answer,
        Some(Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        })
    )
}

#[tokio::test]
async fn a_stand_in_without_a_token_the_gateway_issued_is_refused_and_starts_nothing() {
    // A server that leaves a mark when it is started, and does nothing else.
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("started");
    let marking = StdioMcpServer {
        name: "marking".into(),
        command: PathBuf::from("/usr/bin/touch"),
        args: vec![mark.to_str().unwrap().into()],
    };
    let (relay, _, grants) = relay_for(vec![marking.clone()]);
    let (_grant, token) = granted(&grants, "conversation");
    // None, forged, and one a character off: refused, and nothing started.
    let mut off = token.clone();
    off.replace_range(0..1, if token.starts_with('0') { "1" } else { "0" });
    for forged in [String::new(), "0".repeat(64), off] {
        assert!(unknown_session(answered(&relay, &marking, &forged).await));
    }
    assert!(!mark.exists(), "a server was started for a forged token");
    // Refused before anything is said of the server: an unknown one too.
    let other = StdioMcpServer {
        name: "other".into(),
        ..marking.clone()
    };
    assert!(unknown_session(answered(&relay, &other, "forged").await));
    // The issued token does start it — which is what the mark shows.
    assert!(!unknown_session(answered(&relay, &marking, &token).await));
    assert!(mark.exists());
}

#[tokio::test]
async fn a_token_maps_to_the_conversation_it_was_issued_for_while_its_grant_lives() {
    let (_, _, grants) = relay_for(vec![fixture()]);
    let (first, token) = granted(&grants, "a");
    let (_second, other) = granted(&grants, "b");
    assert_ne!(token, other);
    assert_eq!(token.len(), 64);
    assert_eq!(
        grants.owner(&token).unwrap().session(),
        &SessionId::new("a").unwrap()
    );
    assert_eq!(
        grants.owner(&other).unwrap().session(),
        &SessionId::new("b").unwrap()
    );
    assert_eq!(grants.live(), 2);
    // The grant dropped, its token is stale.
    drop(first);
    assert_eq!(grants.owner(&token), None);
    assert_eq!(grants.live(), 1);
}

/// A random source that has none to give.
struct NoRandom;
impl TokenSource for NoRandom {
    fn fill(&self, _: &mut [u8; 32]) -> Result<(), String> {
        Err("no entropy".into())
    }
}

#[tokio::test]
async fn without_a_token_an_opens_stand_ins_are_refused() {
    let server = fixture();
    let (_, mcp, _) = relay_for(vec![server.clone()]);
    let grants = ConversationGrants::new(mcp.clone(), Arc::new(NoRandom));
    let grant = grants.grant(&SessionId::new("conversation").unwrap());
    // No token to carry, and none registered: its stand-ins say none, and
    // none is let through.
    assert!(grant.environment().is_empty());
    assert_eq!(grants.live(), 0);
    let relay = Arc::new(Relay::new(mcp, grants, key()));
    assert!(unknown_session(answered(&relay, &server, "").await));
    drop(grant);
}

#[test]
fn an_opening_refused_for_a_revoked_grant_is_unknown_session_and_anything_else_unavailable() {
    use super::infrastructure::opening_refused;
    use nessa_sdk::infrastructure::mcp::McpError;
    let (reason, message) = opening_refused(McpError::Closed);
    assert_eq!(reason, StandInRefusal::UnknownSession);
    assert!(message.contains("no open conversation"), "{message}");
    for error in [
        McpError::Timeout,
        McpError::ServerGone,
        McpError::Start("not found".into()),
        McpError::Stopped,
    ] {
        let said = error.to_string();
        let (reason, message) = opening_refused(error);
        assert_eq!(reason, StandInRefusal::Unavailable, "{said}");
        assert_eq!(message, said);
    }
}

#[test]
fn a_hello_never_prints_its_token() {
    let hello = Hello {
        server: "fixture".into(),
        configuration: "sha256:c".into(),
        session: "secret-token".into(),
    };
    let printed = format!("{hello:?}");
    assert!(!printed.contains("secret-token"), "{printed}");
    assert!(printed.contains("fixture"));
}

#[tokio::test]
async fn a_revoked_grant_refuses_its_token_and_ends_the_sessions_it_opened() {
    let server = fixture();
    let (side, _, grants) = relay_for(vec![server.clone()]);
    let (grant, token) = granted(&grants, "conversation");
    let (stand_in, gateway) = tokio::io::duplex(1024 * 1024);
    let relay_side = side.clone();
    tokio::spawn(async move { relay_side.serve(gateway).await });
    let (mut harness_in, input) = tokio::io::duplex(64 * 1024);
    let (output, harness_out) = tokio::io::duplex(64 * 1024);
    let configuration = digest(&server);
    let issued = token.clone();
    let relayed = tokio::spawn(async move {
        relay(stand_in, "fixture", &configuration, &issued, input, output).await
    });
    let mut answers = BufReader::new(harness_out).lines();
    let pid = place(&mut harness_in, &mut answers).await;
    // The conversation's provider session ends: its grant goes.
    drop(grant);
    // Its stand-in ends, its server is closed as the stand-in ending would
    // close it, and its token is refused from now on.
    let relayed = tokio::time::timeout(std::time::Duration::from_secs(10), relayed)
        .await
        .expect("the stand-in ends");
    assert_eq!(relayed.unwrap(), Ok(()));
    gone(pid).await;
    assert!(unknown_session(answered(&side, &server, &token).await));
    drop(harness_in);
}

#[tokio::test]
async fn a_resumed_conversation_gets_a_new_session_and_two_conversations_their_own() {
    let server = fixture();
    let (side, _, grants) = relay_for(vec![server.clone()]);
    let (first, a) = granted(&grants, "a");
    let (_other, b) = granted(&grants, "b");
    let first_pid = through(&side, &server, &a).await;
    let other_pid = through(&side, &server, &b).await;
    assert_ne!(first_pid, other_pid, "each conversation its own server");
    // Resumed: a new grant, a new token, a new session; the old grant's
    // session ends with it, the other conversation's does not.
    let (_resumed, again) = granted(&grants, "a");
    assert_ne!(again, a);
    let resumed_pid = through(&side, &server, &again).await;
    assert_ne!(resumed_pid, first_pid);
    drop(first);
    gone(first_pid).await;
    // SAFETY: signal 0 only asks whether the process exists.
    assert_eq!(unsafe { libc::kill(other_pid, 0) }, 0);
    assert_eq!(unsafe { libc::kill(resumed_pid, 0) }, 0);
}

/// #391 S11, S12: the relay admits each hello against the live set as it is
/// then. A stand-in already serving keeps its server process; an edited
/// server's old stand-in is refused `configuration-changed` on its next
/// hello while the new configuration is let through, and a removed server's
/// is refused `unknown-server`.
#[tokio::test]
async fn a_replaced_set_refuses_old_stand_ins_and_leaves_running_ones_alone() {
    let server = fixture();
    let (side, mcp, grants) = relay_for(vec![server.clone()]);
    let (_grant, token) = granted(&grants, "conversation");
    let running = through(&side, &server, &token).await;
    let edited = StdioMcpServer {
        args: [vec!["-u".to_owned()], server.args.clone()].concat(),
        ..server.clone()
    };
    let launch = |server: &StdioMcpServer| McpServerLaunch {
        server: server.clone(),
        working_directory: std::env::temp_dir(),
        environment: BTreeMap::new(),
    };
    mcp.replace(vec![launch(&edited)]).unwrap();
    assert!(matches!(
        answered(&side, &server, &token).await,
        Some(Answer::Refused {
            reason: Refusal::ConfigurationChanged,
            ..
        })
    ));
    let replaced = through(&side, &edited, &token).await;
    assert_ne!(replaced, running);
    // SAFETY: signal 0 only asks whether the process exists.
    assert_eq!(
        unsafe { libc::kill(running, 0) },
        0,
        "the running harness's server"
    );
    mcp.replace(Vec::new()).unwrap();
    assert!(matches!(
        answered(&side, &edited, &token).await,
        Some(Answer::Refused {
            reason: Refusal::UnknownServer,
            ..
        })
    ));
    assert_eq!(unsafe { libc::kill(replaced, 0) }, 0);
}

/// #391 decision 1: a stand-in admitted against one configuration is never
/// served by another under the same name. A replacement landing between its
/// hello's admission and its opening refuses the opening as the admission
/// would refuse it now: `configuration-changed` for an edit, `unknown-server`
/// for a removal.
#[tokio::test]
async fn a_replacement_between_admission_and_opening_refuses_the_opening() {
    let server = fixture();
    let (side, mcp, grants) = relay_for(vec![server.clone()]);
    let (_grant, token) = granted(&grants, "conversation");
    let hello = Hello {
        server: server.name.clone(),
        configuration: digest(&server),
        session: token.clone(),
    };
    let launch = |server: &StdioMcpServer| McpServerLaunch {
        server: server.clone(),
        working_directory: std::env::temp_dir(),
        environment: BTreeMap::new(),
    };
    let edited = StdioMcpServer {
        args: [vec!["-u".to_owned()], server.args.clone()].concat(),
        ..server.clone()
    };
    let admitted = side.admitted(&hello).unwrap();
    mcp.replace(vec![launch(&edited)]).unwrap();
    let owner = grants.owner(&token).unwrap();
    assert!(matches!(
        side.open_admitted(&admitted, owner.clone()).await,
        Err((StandInRefusal::ConfigurationChanged, _))
    ));
    let admitted = side
        .admitted(&Hello {
            configuration: digest(&edited),
            ..hello
        })
        .unwrap();
    mcp.replace(Vec::new()).unwrap();
    assert!(matches!(
        side.open_admitted(&admitted, owner).await,
        Err((StandInRefusal::UnknownServer, _))
    ));
}

/// Through the relay with `token`, list and call `where`: the server's pid.
/// The stand-in is left running, its harness's ends held open.
async fn through(side: &Arc<Relay>, server: &StdioMcpServer, token: &str) -> libc::pid_t {
    let (stand_in, gateway) = tokio::io::duplex(1024 * 1024);
    let relay_side = side.clone();
    tokio::spawn(async move { relay_side.serve(gateway).await });
    let (mut harness_in, input) = tokio::io::duplex(64 * 1024);
    let (output, harness_out) = tokio::io::duplex(64 * 1024);
    let configuration = digest(server);
    let token = token.to_owned();
    tokio::spawn(
        async move { relay(stand_in, "fixture", &configuration, &token, input, output).await },
    );
    let mut answers = BufReader::new(harness_out).lines();
    let pid = place(&mut harness_in, &mut answers).await;
    // Held open for the test's life.
    std::mem::forget((harness_in, answers));
    pid
}

/// List, then call `where`, as a harness would: the server's pid.
async fn place(
    harness_in: &mut DuplexStream,
    answers: &mut tokio::io::Lines<BufReader<DuplexStream>>,
) -> libc::pid_t {
    let mut answer = Value::Null;
    for message in [
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "where" } }),
    ] {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        harness_in.write_all(&bytes).await.unwrap();
        // Each answered before the next is asked: a call before its list is
        // answered is refused.
        answer = serde_json::from_str(&next_line(answers).await).unwrap();
    }
    answer["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap() as libc::pid_t
}

/// The next line the stand-in answers, within ten seconds.
async fn next_line(answers: &mut tokio::io::Lines<BufReader<DuplexStream>>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(10), answers.next_line())
        .await
        .expect("an answer within ten seconds")
        .unwrap()
        .expect("a line, not the end")
}

/// Until `pid` has exited, within five seconds.
async fn gone(pid: libc::pid_t) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("process {pid} is still running"));
}

#[tokio::test]
async fn a_server_that_cannot_start_refuses_its_stand_in_as_unavailable() {
    let missing = StdioMcpServer {
        name: "missing".into(),
        command: PathBuf::from("/nonexistent/server"),
        args: vec![],
    };
    let (relay, _, grants) = relay_for(vec![missing.clone()]);
    let (_grant, token) = granted(&grants, "conversation");
    let (mut read, mut write) = connect(&relay);
    hello(&mut write, "missing", &digest(&missing), &token).await;
    let Some(Answer::Refused { reason, message }) = read_line::<Answer>(&mut read).await else {
        panic!("refused");
    };
    assert_eq!(reason, Refusal::Unavailable);
    assert!(message.contains("could not be started"), "{message}");
}

#[tokio::test]
async fn a_hello_that_is_not_one_bounded_json_line_is_closed_without_an_answer() {
    let (relay, _, _) = relay_for(vec![fixture()]);
    for bytes in [
        b"not json\n".to_vec(),
        b"{\"server\":\"fixture\",\"configuration\":\"x\",\"extra\":1}\n".to_vec(),
        vec![b' '; MAX_HELLO_BYTES + 1],
        // A hello that would be read, but only past the bound.
        [
            vec![b' '; MAX_HELLO_BYTES],
            b"{\"server\":\"fixture\",\"configuration\":\"x\",\"session\":\"x\"}\n".to_vec(),
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
    let (relay, _, _) = relay_for(vec![fixture()]);
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
    let result = relay(
        scripted(Some(refused)),
        "s",
        "c",
        "t",
        input,
        tokio::io::sink(),
    )
    .await;
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
    let result = relay(scripted(None), "s", "c", "t", input, tokio::io::sink()).await;
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
        "t",
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
    let running = tokio::time::timeout(std::time::Duration::from_secs(10), running)
        .await
        .expect("the stand-in ends");
    assert_eq!(running.unwrap(), Ok(()));
}

/// #435: an open's grant holds the very results the stand-ins its token lets
/// through keep — those of the owner the relay resolves that token to — and
/// another open's grant, of the same conversation, holds others.
#[test]
fn an_opens_grant_holds_what_the_stand_ins_under_its_token_forward() {
    let (_, _, grants) = relay_for(vec![fixture()]);
    let (grant, token) = granted(&grants, "conversation");
    let (other, other_token) = granted(&grants, "conversation");
    let owner = grants.owner(&token).expect("a live grant");
    assert_eq!(grant.forwarded(), Some(&owner.forwarded()));
    assert_ne!(other.forwarded(), Some(&owner.forwarded()));
    assert_eq!(
        other.forwarded(),
        Some(&grants.owner(&other_token).unwrap().forwarded())
    );
}

/// A harness reaches the real server through `mcp-relay` and the relay
/// socket, on a session of its own, and the view's lookup reads that
/// session's list; the session ends with the stand-in.
#[tokio::test]
async fn a_harness_through_the_relay_gets_a_session_of_its_own() {
    let server = fixture();
    let (relay_side, mcp, grants) = relay_for(vec![server.clone()]);
    const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000aa";
    let (_grant, token) = granted(&grants, CONVERSATION);
    let conversation = ConversationId::new(CONVERSATION).unwrap();
    let (stand_in, gateway) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(async move { relay_side.serve(gateway).await });
    let (mut harness_in, input) = tokio::io::duplex(64 * 1024);
    let (output, harness_out) = tokio::io::duplex(64 * 1024);
    let configuration = digest(&server);
    let relayed = tokio::spawn(async move {
        relay(stand_in, "fixture", &configuration, &token, input, output).await
    });
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
    let initialized: Value = serde_json::from_str(&next_line(&mut answers).await).unwrap();
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
    next_line(&mut answers).await;
    harness_in
        .write_all(&ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "where" } })))
        .await
        .unwrap();
    let place: Value = serde_json::from_str(&next_line(&mut answers).await).unwrap();
    let pid = place["result"]["structuredContent"]["pid"]
        .as_i64()
        .unwrap() as libc::pid_t;
    // The view's lookup reads the conversation's session's list, once it has
    // been read; another conversation sees none of it.
    let uis = ListedToolUis(mcp.clone());
    let chart = McpTool::new("fixture", "show_chart").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while uis.resource_uri(&conversation, &chart).is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the session's tools are listed");
    assert_eq!(
        uis.resource_uri(&conversation, &chart).unwrap().as_str(),
        "ui://fixture/chart.html"
    );
    assert_eq!(
        uis.resource_uri(
            &ConversationId::new("00000000-0000-4000-8000-0000000000bb").unwrap(),
            &chart
        ),
        None
    );
    assert_eq!(
        uis.resource_uri(&conversation, &McpTool::new("fixture", "remember").unwrap()),
        None
    );
    // The harness goes; its stand-in and session end, and the server with them.
    drop(harness_in);
    let relayed = tokio::time::timeout(std::time::Duration::from_secs(10), relayed)
        .await
        .expect("the stand-in ends");
    assert_eq!(relayed.unwrap(), Ok(()));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // SAFETY: signal 0 only asks whether the process exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the server ends with its stand-in");
    assert_eq!(uis.resource_uri(&conversation, &chart), None);
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
