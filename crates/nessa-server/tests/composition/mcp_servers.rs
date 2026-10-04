//! Composing the gateway's one connection per MCP server: what every agent is
//! handed in place of a server, read from the live set at each open, what a
//! server is started with, the relay composed with no server configured, and
//! a relay socket that cannot be bound leaving MCP off rather than handing
//! agents the servers themselves.
use super::super::agent::AgentsConfig;
use super::{compose, relay_socket, server_environment, stand_ins, StandIns};
use crate::mcp_servers::domain::configuration_digest;
use nessa_sdk::infrastructure::{
    acp::sessions::StdioMcpServer,
    clock::RuntimeClock,
    mcp::{McpServerLaunch, McpServers},
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
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
        mcp_stand_ins: Default::default(),
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
    let gateway = "/bundle/nessa";
    let socket = "/tmp/nessa-mcp-501/0123456789abcdef.sock";
    let handed = stand_ins(&configured, gateway, socket);
    for (stand_in, server) in handed.iter().zip(&configured) {
        assert_eq!(stand_in.name, server.name);
        assert_eq!(stand_in.command, Path::new(gateway));
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
    // another, so the relay, which compares the digest, refuses a server
    // changed under an open conversation with `configuration-changed`. The
    // restoration fingerprint does not read these (ADR 344, #391).
    assert_eq!(stand_ins(&configured, gateway, socket), handed);
    let changed = vec![server("mcptest", &["/other.mjs"]), configured[1].clone()];
    assert_ne!(stand_ins(&changed, gateway, socket)[0].args, handed[0].args);
    assert_eq!(stand_ins(&changed, gateway, socket)[1].args, handed[1].args);
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

/// The live set of `servers`, started in `workspace` with nothing in their
/// environment.
fn live(servers: &[StdioMcpServer]) -> McpServers {
    McpServers::new(launches(servers), Arc::new(RuntimeClock::new())).unwrap()
}

fn launches(servers: &[StdioMcpServer]) -> Vec<McpServerLaunch> {
    servers
        .iter()
        .map(|server| McpServerLaunch {
            server: server.clone(),
            working_directory: std::env::temp_dir(),
            environment: BTreeMap::new(),
        })
        .collect()
}

#[test]
fn a_gateway_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let gateway = PathBuf::from(std::ffi::OsStr::from_bytes(b"/app/\xff/nessa"));
    let servers = live(&[server("s", &[])]);
    assert!(StandIns::new(servers, &gateway, Path::new("/tmp/s.sock")).is_none());
}

#[test]
fn a_socket_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let socket = PathBuf::from(std::ffi::OsStr::from_bytes(b"/data/\xff/relay.sock"));
    let servers = live(&[server("s", &[])]);
    assert!(StandIns::new(servers, Path::new("/nessa"), &socket).is_none());
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

/// The names of the stand-ins a provider open of `config` is given now.
fn opened(config: &AgentsConfig) -> Vec<String> {
    config
        .mcp_stand_ins
        .opened()
        .current()
        .iter()
        .map(|stand_in| stand_in.name.clone())
        .collect()
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
    assert_eq!(composed.servers.configured(), vec![configured.clone()]);
    // Taken into the live set: the configuration keeps no copy.
    assert!(config.mcp_servers.is_empty());
    let handed = config.mcp_stand_ins.opened().current();
    assert_eq!(handed[0].command, Path::new("/nessa"));
    assert_eq!(handed[0].args[0], "mcp-relay");
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
        .find(|(name, _)| name == nessa_sdk::infrastructure::mcp::MCP_SESSION_VARIABLE)
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
    assert!(opened(&config).is_empty());
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
    assert!(opened(&config).is_empty());
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

/// A hello for `server` naming `token` is refused for `reason`.
async fn refused(
    composed: &super::McpComposition,
    server: &StdioMcpServer,
    token: &str,
    reason: crate::mcp_servers::infrastructure::Refusal,
) -> bool {
    use crate::mcp_servers::infrastructure::Answer;
    matches!(
        answered(composed, server, token).await,
        Answer::Refused { reason: said, .. } if said == reason
    )
}

/// A token one open of `config` carries, and the grant that keeps it live.
fn token(
    config: &AgentsConfig,
) -> (
    nessa_sdk::infrastructure::acp::sessions::StandInGrant,
    String,
) {
    let (opened, grant) = config.stand_ins.opened(Some(
        &nessa_sdk::domain::agent_execution::sessions::SessionId::new("conversation").unwrap(),
    ));
    let token = opened.environment()[0].1.clone();
    (grant.expect("a grant"), token)
}

/// #391 S18: with no server configured the relay is composed all the same,
/// so a server added later reaches the next open and is let through.
#[tokio::test]
async fn s18_with_no_server_configured_the_relay_exists_and_a_server_added_reaches_the_next_open() {
    use crate::mcp_servers::infrastructure::Refusal;
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let mut config = agents(Vec::new());
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("the relay is composed with no server configured");
    assert!(opened(&config).is_empty());
    let added = server("mcptest", &["/s.mjs"]);
    let (_grant, token) = token(&config);
    assert!(refused(&composed, &added, &token, Refusal::UnknownServer).await);
    composed
        .servers
        .replace(launches(std::slice::from_ref(&added)))
        .unwrap();
    assert_eq!(opened(&config), ["mcptest"]);
    // Admitted: this test's server cannot start, so it is refused as
    // unavailable rather than unknown.
    assert!(refused(&composed, &added, &token, Refusal::Unavailable).await);
}

/// #391 S11–S13 through the composed gateway: an edited server's old
/// stand-in is refused `configuration-changed` and a new open gets the new
/// one; a removed server's is refused `unknown-server` and a new open does
/// not list it; added back, it is in the next open again.
#[tokio::test]
async fn s11_to_s13_a_replaced_set_reaches_the_next_open_and_old_stand_ins_are_refused() {
    use crate::mcp_servers::infrastructure::Refusal;
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    let original = server("mcptest", &["/s.mjs"]);
    let mut config = agents(vec![original.clone()]);
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    let (_grant, token) = token(&config);
    let before = config.mcp_stand_ins.opened().current();
    // S11: edited.
    let edited = server("mcptest", &["/edited.mjs"]);
    composed
        .servers
        .replace(launches(std::slice::from_ref(&edited)))
        .unwrap();
    assert!(refused(&composed, &original, &token, Refusal::ConfigurationChanged).await);
    assert!(refused(&composed, &edited, &token, Refusal::Unavailable).await);
    let after = config.mcp_stand_ins.opened().current();
    assert_eq!(after[0].name, "mcptest");
    assert_ne!(after[0].args, before[0].args);
    // S12: removed.
    composed.servers.replace(Vec::new()).unwrap();
    assert!(refused(&composed, &edited, &token, Refusal::UnknownServer).await);
    assert!(opened(&config).is_empty());
    // S13: back again.
    composed
        .servers
        .replace(launches(std::slice::from_ref(&edited)))
        .unwrap();
    assert_eq!(opened(&config), ["mcptest"]);
    assert!(refused(&composed, &edited, &token, Refusal::Unavailable).await);
}
