//! Composing the gateway's one connection per MCP server: what every agent is
//! handed in place of a server, what a server is started with, and a relay
//! socket that cannot be bound leaving MCP off rather than handing agents the
//! servers themselves.
use super::super::agent::AgentsConfig;
use super::{compose, relay_socket, server_environment, stand_ins};
use crate::mcp_servers::domain::configuration_digest;
use nessa_sdk::infrastructure::acp::sessions::StdioMcpServer;
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
};

fn server(name: &str, args: &[&str]) -> StdioMcpServer {
    StdioMcpServer {
        name: name.into(),
        command: PathBuf::from("/usr/bin/python3"),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

fn agents(servers: Vec<StdioMcpServer>) -> AgentsConfig {
    AgentsConfig {
        catalog: PathBuf::from("/runtime/models.json"),
        workspace: std::env::temp_dir(),
        mcp_servers: servers,
        stand_ins: Default::default(),
        selected: None,
        runtimes: HashMap::new(),
    }
}

#[test]
fn each_server_is_handed_over_as_a_relay_under_its_own_name() {
    let configured = vec![
        server("mcptest", &["/s.mjs"]),
        server("nessa", &["--workspace", "/w"]),
    ];
    let gateway = Path::new("/bundle/nessa");
    let socket = Path::new("/tmp/nessa-mcp-501/0123456789abcdef.sock");
    let handed = stand_ins(&configured, gateway, socket).unwrap();
    for (stand_in, server) in handed.iter().zip(&configured) {
        assert_eq!(stand_in.name, server.name);
        assert_eq!(stand_in.command, gateway);
        assert_eq!(
            stand_in.args,
            [
                "mcp-relay",
                "/tmp/nessa-mcp-501/0123456789abcdef.sock",
                server.name.as_str(),
                configuration_digest(&server.command, &server.args).as_str(),
            ]
        );
    }
    // The same configuration is the same stand-in, run after run; another is
    // another, so a harness's context fingerprint sees the change.
    assert_eq!(stand_ins(&configured, gateway, socket).unwrap(), handed);
    let changed = vec![server("mcptest", &["/other.mjs"]), configured[1].clone()];
    assert_ne!(
        stand_ins(&changed, gateway, socket).unwrap()[0].args,
        handed[0].args
    );
    assert_eq!(
        stand_ins(&changed, gateway, socket).unwrap()[1].args,
        handed[1].args
    );
}

#[test]
fn the_relay_socket_is_short_per_user_and_the_same_for_one_namespace() {
    let socket = relay_socket(Path::new("/Users/me/.nessa/dev"), 501);
    let name = socket.file_name().unwrap().to_str().unwrap();
    assert_eq!(socket.parent().unwrap(), Path::new("/tmp/nessa-mcp-501"));
    assert!(name.len() == 16 + 5 && name.ends_with(".sock"), "{name}");
    assert_eq!(relay_socket(Path::new("/Users/me/.nessa/dev"), 501), socket);
    assert_ne!(relay_socket(Path::new("/Users/me/.nessa/ci"), 501), socket);
    assert_ne!(relay_socket(Path::new("/Users/me/.nessa/dev"), 502), socket);
    // However long the namespace, the socket fits the platform's limit.
    let deep = PathBuf::from(format!("/{}", "d".repeat(4000)));
    assert!(relay_socket(&deep, u32::MAX).as_os_str().len() < 104);
}

#[test]
fn a_gateway_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let gateway = PathBuf::from(std::ffi::OsStr::from_bytes(b"/app/\xff/nessa"));
    assert_eq!(
        stand_ins(&[server("s", &[])], &gateway, Path::new("/tmp/s.sock")),
        None
    );
}

#[test]
fn a_socket_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let socket = PathBuf::from(std::ffi::OsStr::from_bytes(b"/data/\xff/relay.sock"));
    assert_eq!(
        stand_ins(&[server("s", &[])], Path::new("/nessa"), &socket),
        None
    );
}

#[test]
fn a_server_is_started_with_the_users_variables_and_the_agents_search_path() {
    let variables = BTreeMap::from([
        ("HOME", "/home/me"),
        ("USER", "me"),
        ("LANG", "en_US.UTF-8"),
        ("PATH", "/usr/bin"),
        ("NESSA_AGENT_PATH", "/login/shell/path"),
        ("ANTHROPIC_API_KEY", "secret"),
        ("CODEX_HOME", "/codex"),
    ]);
    let lookup = |key: &str| variables.get(key).map(OsString::from);
    let environment = server_environment(lookup);
    let expected: BTreeMap<OsString, OsString> = [
        ("HOME", "/home/me"),
        ("USER", "me"),
        ("LANG", "en_US.UTF-8"),
        // The agents' search path, as the Nessa shell tool had it under an agent.
        ("PATH", "/login/shell/path"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    assert_eq!(environment, expected);
    // Without the host's search path, the process's own.
    let without = |key: &str| (key == "PATH").then(|| OsString::from("/usr/bin"));
    assert_eq!(
        server_environment(without),
        BTreeMap::from([(OsString::from("PATH"), OsString::from("/usr/bin"))])
    );
}

#[tokio::test]
async fn no_configured_server_composes_nothing() {
    let mut config = agents(Vec::new());
    let composed = compose(
        &mut config,
        &std::env::temp_dir(),
        Path::new("/nessa"),
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert!(composed.is_none());
    assert!(config.mcp_servers.is_empty());
}

#[tokio::test]
async fn the_agents_get_stand_ins_and_the_relay_is_bound_privately() {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let configured = server("mcptest", &["/s.mjs"]);
    let mut config = agents(vec![configured.clone()]);
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    assert_eq!(composed.servers.names().collect::<Vec<_>>(), ["mcptest"]);
    assert_eq!(config.mcp_servers[0].command, Path::new("/nessa"));
    assert_eq!(config.mcp_servers[0].args[0], "mcp-relay");
    assert!(std::fs::symlink_metadata(&socket)
        .unwrap()
        .file_type()
        .is_socket());
    let directory = std::fs::metadata(socket.parent().unwrap()).unwrap();
    assert_eq!(directory.permissions().mode() & 0o777, 0o700);
    // A second run replaces the socket the first left behind, once the first
    // has let go — allowing an instant: a child another test forks just then
    // holds the lock until it execs.
    drop(composed);
    let started = std::time::Instant::now();
    loop {
        let mut again = agents(vec![configured.clone()]);
        let composed = compose(&mut again, &socket, Path::new("/nessa"), BTreeMap::new())
            .await
            .unwrap();
        if composed.is_some() {
            break;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// What the composed relay answers a hello naming `token` for `server`.
async fn answered(
    composed: &super::McpComposition,
    server: &StdioMcpServer,
    token: &str,
) -> crate::mcp_servers::infrastructure::Answer {
    use crate::mcp_servers::infrastructure::{read_line, write_line, Hello};
    let (stand_in, gateway) = tokio::io::duplex(64 * 1024);
    let relay = composed.relay.clone();
    tokio::spawn(async move { relay.serve(gateway).await });
    let (read, mut write) = tokio::io::split(stand_in);
    let hello = Hello {
        server: server.name.clone(),
        configuration: configuration_digest(&server.command, &server.args),
        session: token.into(),
    };
    write_line(&mut write, &hello).await.unwrap();
    read_line(&mut tokio::io::BufReader::new(read))
        .await
        .expect("an answer")
}

#[tokio::test]
async fn the_agents_grants_are_the_ones_the_composed_relay_lets_through() {
    use crate::mcp_servers::infrastructure::{Answer, Refusal};
    use nessa_sdk::domain::agent_execution::sessions::SessionId;
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let configured = server("mcptest", &["/s.mjs"]);
    let mut config = agents(vec![configured.clone()]);
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    // An open of a conversation's session, as its provider makes it.
    let (opened, grant) = config
        .stand_ins
        .opened(Some(&SessionId::new("conversation").unwrap()));
    let token = opened
        .environment()
        .iter()
        .find(|(name, _)| name == crate::mcp_servers::domain::SESSION_VARIABLE)
        .map(|(_, token)| token.clone())
        .expect("the agents' opens carry a token");
    // Let past the session check (this test's server cannot start, so it is
    // refused as unavailable instead); a forged one is not.
    assert!(!matches!(
        answered(&composed, &configured, &token).await,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
    assert!(matches!(
        answered(&composed, &configured, "forged").await,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
    // Revoked with its open, it is refused too.
    drop(grant);
    assert!(matches!(
        answered(&composed, &configured, &token).await,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
}

#[tokio::test]
async fn a_relay_that_cannot_be_bound_leaves_mcp_servers_off() {
    let namespace = tempfile::tempdir().unwrap();
    // Something that is not a socket where the socket goes is left alone.
    let socket = namespace.path().join("mcp").join("relay.sock");
    nessa_local_storage::create_directory(socket.parent().unwrap()).unwrap();
    std::fs::write(&socket, b"not a socket").unwrap();
    assert_eq!(
        crate::mcp_servers::infrastructure::bind(&socket)
            .await
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let mut config = agents(vec![server("mcptest", &[])]);
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap();
    assert!(composed.is_none());
    // Off, not handed over directly.
    assert!(config.mcp_servers.is_empty());
    assert_eq!(std::fs::read(&socket).unwrap(), b"not a socket");
    // A path past the platform's socket limit is the same.
    let deep = namespace.path().join("d".repeat(120)).join("relay.sock");
    let mut config = agents(vec![server("mcptest", &[])]);
    assert!(
        compose(&mut config, &deep, Path::new("/nessa"), BTreeMap::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(config.mcp_servers.is_empty());
}

#[tokio::test]
async fn servers_that_cannot_be_launched_as_configured_are_an_agent_error() {
    let mut config = agents(vec![server("a__b", &[])]);
    assert!(matches!(
        compose(
            &mut config,
            &std::env::temp_dir(),
            Path::new("/nessa"),
            BTreeMap::new()
        )
        .await,
        Err(crate::core::RunError::Agent(_))
    ));
}

/// An audit that keeps what it commits.
#[derive(Default)]
struct KeptAudit(std::sync::Mutex<Vec<crate::conversation::application::McpAppAuditRecord>>);
impl crate::conversation::application::McpAppAudit for KeptAudit {
    fn record(
        &self,
        record: crate::conversation::application::McpAppAuditRecord,
    ) -> crate::conversation::application::ConversationFuture<'_, ()> {
        self.0.lock().unwrap().push(record);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_composed_gateways_apps_drops_are_written_to_its_audit_before_it_exits() {
    use crate::conversation::application::{
        ContextDrop, McpAppAsk, McpAppAuditPhase, McpAppAuditRecord, McpAppInitiator, McpAppRef,
    };
    use std::sync::Arc;
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let mut config = agents(vec![server("mcptest", &["/s.mjs"])]);
    let mut composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    // As composition starts them, beside the conversation service it hands
    // the sink to; and as the gateway's exit finishes them.
    let audit = Arc::new(KeptAudit::default());
    let dropped = composed.start_recorders(audit.clone());
    let drop = McpAppAuditRecord {
        conversation_id: crate::conversation::domain::ConversationId::new(
            "00000000-0000-4000-8000-000000000001",
        )
        .unwrap(),
        organization_id: nessa_auth::domain::OrganizationId::new("org").unwrap(),
        call_id: "u1".into(),
        request_id: "request-u1".into(),
        app: McpAppRef {
            execution_id: "e1".into(),
            tool_id: "t1".into(),
            instance_id: "i1".into(),
        },
        ask: McpAppAsk::UpdateModelContext {
            server: "mcptest".into(),
        },
        initiator: McpAppInitiator::System,
        phase: McpAppAuditPhase::ContextDropped {
            cause: ContextDrop::ConversationEnded,
        },
    };
    dropped.context_dropped(drop.clone());
    super::finish_recorders(
        [
            composed.ticket_recorder.take(),
            composed.context_drop_recorder.take(),
        ]
        .into_iter()
        .flatten(),
        super::RECORDERS_FINISH,
    )
    .await;
    assert_eq!(*audit.0.lock().unwrap(), [drop]);
}

#[tokio::test(start_paused = true)]
async fn recorders_finish_together_under_one_bound() {
    // Two recorders, each still recording at the bound.
    let hanging = |what| {
        let (stop, _stopping) = tokio::sync::oneshot::channel();
        super::AuditRecorder {
            what,
            stop,
            task: tokio::spawn(std::future::pending()),
        }
    };
    let started = tokio::time::Instant::now();
    super::finish_recorders(
        [hanging("ticket ends"), hanging("context drops")],
        super::RECORDERS_FINISH,
    )
    .await;
    // Together, not one after the other: the exit waits the one bound.
    assert_eq!(started.elapsed(), super::RECORDERS_FINISH);
}

/// The drop sink `mcp_app_ports` builds for the conversation service's
/// `McpAppPorts` is the composed recorder's: a context dropped by the
/// service's own close is written to the audit that recorder was started
/// with, by the closer, before the exit's finish returns. Any other sink
/// built there writes nothing to it. This pins the factory, not
/// `local_auth`'s one-expression call of it.
#[tokio::test]
async fn a_composed_gateways_dropped_context_is_written_by_its_recorder() {
    use crate::app_call_test_support::{caller, Fixture, INSTANCE, SERVER};
    use crate::conversation::application::{
        ContextDrop, McpAppAsk, McpAppAuditPhase, McpAppInitiator,
    };
    use std::sync::Arc;
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let mut config = agents(vec![server(SERVER, &["/s.mjs"])]);
    let mut composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    let audit = Arc::new(KeptAudit::default());
    let ports = super::super::local_auth::mcp_app_ports(&mut composed, audit.clone());
    let fixture = Fixture::dropping_to(ports.dropped).await;
    fixture
        .update_context(INSTANCE, Some("Showing April"), None)
        .await
        .unwrap();
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    super::finish_recorders(
        [
            composed.ticket_recorder.take(),
            composed.context_drop_recorder.take(),
        ]
        .into_iter()
        .flatten(),
        super::RECORDERS_FINISH,
    )
    .await;
    let written = audit.0.lock().unwrap().clone();
    assert_eq!(written.len(), 1, "{written:?}");
    let drop = &written[0];
    assert_eq!(drop.conversation_id, fixture.id);
    assert_eq!(drop.request_id, "app-context");
    assert_eq!(drop.app, fixture.app(INSTANCE));
    assert_eq!(
        drop.ask,
        McpAppAsk::UpdateModelContext {
            server: SERVER.into()
        }
    );
    assert_eq!(
        drop.phase,
        McpAppAuditPhase::ContextDropped {
            cause: ContextDrop::ConversationEnded
        }
    );
    assert!(
        matches!(&drop.initiator, McpAppInitiator::Person { request_id, .. } if request_id == "close"),
        "{:?}",
        drop.initiator
    );
}
