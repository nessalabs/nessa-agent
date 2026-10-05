//! Composing the gateway's one connection per MCP server: what every agent is
//! handed in place of a server, read from the live set at each open, what a
//! server is started with, the relay composed with no server configured, and
//! a relay socket that cannot be bound leaving MCP off rather than handing
//! agents the servers themselves.
use super::super::agent::AgentsConfig;
use super::{compose, relay_socket, server_environment, stand_ins, StandIns};
use crate::mcp_servers::domain::{ConfigurationKey, ConfiguredMcpServer, StdioServer};
use crate::mcp_servers::infrastructure::launch_digest;
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
    time::Duration,
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
        mcp_servers: servers
            .into_iter()
            .map(|server| {
                let server = StdioServer::new(server.name, server.command, server.args);
                ConfiguredMcpServer::new(server, true, []).unwrap()
            })
            .collect(),
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
    let key = ConfigurationKey::new([7; 32]);
    let configured = launches(&configured);
    let handed = stand_ins(&configured, gateway, socket, &key);
    for (stand_in, launch) in handed.iter().zip(&configured) {
        assert_eq!(stand_in.name, launch.server.name);
        assert_eq!(stand_in.command, Path::new(gateway));
        assert_eq!(
            stand_in.args,
            [
                "mcp-relay",
                "/tmp/nessa-mcp-501/0123456789abcdef.sock",
                launch.server.name.as_str(),
                launch_digest(&key, launch).as_str(),
            ]
        );
    }
    // The same configuration is the same stand-in for this process; another
    // — its arguments, or only its environment — is another, so the relay,
    // which compares the digest, refuses a server changed under an open
    // conversation with `configuration-changed`. Another process's key gives
    // another stand-in: the restoration fingerprint does not read these
    // (ADR 344, #391).
    assert_eq!(stand_ins(&configured, gateway, socket, &key), handed);
    let mut changed = configured.clone();
    changed[0].server.args = vec!["/other.mjs".into()];
    let mut environment_only = configured.clone();
    environment_only[0]
        .environment
        .insert("API_TOKEN".into(), "rotated".into());
    for edited in [changed, environment_only] {
        let again = stand_ins(&edited, gateway, socket, &key);
        assert_ne!(again[0].args, handed[0].args);
        assert_eq!(again[1].args, handed[1].args);
    }
    let other_key = stand_ins(
        &configured,
        gateway,
        socket,
        &ConfigurationKey::new([8; 32]),
    );
    assert_ne!(other_key[0].args, handed[0].args);
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
    let key = ConfigurationKey::new([7; 32]);
    assert!(StandIns::new(servers, &gateway, Path::new("/tmp/s.sock"), key).is_none());
}

#[test]
fn a_socket_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let socket = PathBuf::from(std::ffi::OsStr::from_bytes(b"/data/\xff/relay.sock"));
    let servers = live(&[server("s", &[])]);
    let key = ConfigurationKey::new([7; 32]);
    assert!(StandIns::new(servers, Path::new("/nessa"), &socket, key).is_none());
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
    assert_eq!(
        composed.servers.configured(),
        launches(std::slice::from_ref(&configured))
    );
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

/// The digest the stand-in for `name` that a provider open of `config` is
/// given now carries.
fn handed_digest(config: &AgentsConfig, name: &str) -> String {
    config
        .mcp_stand_ins
        .opened()
        .current()
        .iter()
        .find(|stand_in| stand_in.name == name)
        .map(|stand_in| stand_in.args[3].clone())
        .expect("a stand-in under the name")
}

/// What the composed relay answers a hello naming `token` for the server
/// `name` with the digest `configuration`.
async fn answered(
    composed: &super::McpComposition,
    name: &str,
    configuration: &str,
    token: &str,
) -> crate::mcp_servers::infrastructure::Answer {
    use crate::mcp_servers::infrastructure::{read_line, write_line, Hello};
    let (stand_in, gateway) = tokio::io::duplex(64 * 1024);
    let relay = composed.relay.clone();
    tokio::spawn(async move { relay.serve(gateway).await });
    let (read, mut write) = tokio::io::split(stand_in);
    let hello = Hello {
        server: name.into(),
        configuration: configuration.into(),
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
    let digest = handed_digest(&config, "mcptest");
    // Let past the session check (this test's server cannot start, so it is
    // refused as unavailable instead); a forged one is not.
    assert!(!matches!(
        answered(&composed, "mcptest", &digest, &token).await,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
    assert!(matches!(
        answered(&composed, "mcptest", &digest, "forged").await,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
    // Revoked with its open, it is refused too.
    drop(grant);
    assert!(matches!(
        answered(&composed, "mcptest", &digest, &token).await,
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

/// Startup refuses servers that cannot be launched as configured, and the
/// error names the server — an entry added to `config.json` by hand among
/// them — whatever its problem.
#[tokio::test]
async fn servers_that_cannot_be_launched_as_configured_are_an_agent_error_naming_the_server() {
    let mut relative = server("hand-added", &[]);
    relative.command = "relative/server".into();
    for (configured, variable, named) in [
        (server("a__b", &[]), None, "\"a__b\""),
        (relative, None, "\"hand-added\""),
        (server("hand-env", &[]), Some("1BAD"), "\"hand-env\""),
    ] {
        let mut config = agents(vec![server("ok", &[]), configured]);
        if let Some(variable) = variable {
            let server = config.mcp_servers[1].server().clone();
            config.mcp_servers[1] =
                ConfiguredMcpServer::new(server, true, [(variable.into(), "value".into())])
                    .unwrap();
        }
        let refused = compose(
            &mut config,
            &std::env::temp_dir(),
            Path::new("/nessa"),
            BTreeMap::new(),
        )
        .await;
        match refused {
            Err(crate::core::RunError::Agent(message)) => {
                assert!(message.contains(named), "{message}")
            }
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("{named} was composed"),
        }
    }
}

/// A hello for the server `name` with the digest `configuration`, naming
/// `token`, is refused for `reason`.
async fn refused(
    composed: &super::McpComposition,
    name: &str,
    configuration: &str,
    token: &str,
    reason: crate::mcp_servers::infrastructure::Refusal,
) -> bool {
    use crate::mcp_servers::infrastructure::Answer;
    matches!(
        answered(composed, name, configuration, token).await,
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
    assert!(refused(&composed, "mcptest", "any", &token, Refusal::UnknownServer).await);
    composed
        .servers
        .replace(launches(std::slice::from_ref(&added)))
        .unwrap();
    assert_eq!(opened(&config), ["mcptest"]);
    // Admitted: this test's server cannot start, so it is refused as
    // unavailable rather than unknown.
    let digest = handed_digest(&config, "mcptest");
    assert!(refused(&composed, "mcptest", &digest, &token, Refusal::Unavailable).await);
}

/// #391 S11–S13 through the composed gateway: an edited server's old
/// stand-in is refused `configuration-changed` and a new open gets the new
/// one — whether its arguments changed or only its environment (pass 2b,
/// decision 2); a removed server's is refused `unknown-server` and a new
/// open does not list it; added back, it is in the next open again.
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
    let before = handed_digest(&config, "mcptest");
    // S11: edited.
    let edited = launches(&[server("mcptest", &["/edited.mjs"])]);
    composed.servers.replace(edited.clone()).unwrap();
    assert!(
        refused(
            &composed,
            "mcptest",
            &before,
            &token,
            Refusal::ConfigurationChanged
        )
        .await
    );
    let after = handed_digest(&config, "mcptest");
    assert_ne!(after, before);
    assert!(refused(&composed, "mcptest", &after, &token, Refusal::Unavailable).await);
    // S11: only its environment edited.
    let mut rotated = edited.clone();
    rotated[0]
        .environment
        .insert("API_TOKEN".into(), "rotated".into());
    composed.servers.replace(rotated).unwrap();
    assert!(
        refused(
            &composed,
            "mcptest",
            &after,
            &token,
            Refusal::ConfigurationChanged
        )
        .await
    );
    let rotated = handed_digest(&config, "mcptest");
    assert_ne!(rotated, after);
    assert!(refused(&composed, "mcptest", &rotated, &token, Refusal::Unavailable).await);
    // S12: removed.
    composed.servers.replace(Vec::new()).unwrap();
    assert!(
        refused(
            &composed,
            "mcptest",
            &rotated,
            &token,
            Refusal::UnknownServer
        )
        .await
    );
    assert!(opened(&config).is_empty());
    // S13: back again.
    composed.servers.replace(edited).unwrap();
    assert_eq!(opened(&config), ["mcptest"]);
    let back = handed_digest(&config, "mcptest");
    assert_eq!(back, after);
    assert!(refused(&composed, "mcptest", &back, &token, Refusal::Unavailable).await);
}

/// What `config.json`'s `agents.mcpServers` parses to at startup: an entry
/// without `enabled` or `env` is on with no variables of its own; one turned
/// off stays configured and is left out of the live set; and the runtime
/// configuration refuses anything else in an entry.
#[tokio::test]
async fn stored_entries_parse_with_their_defaults_and_a_disabled_one_is_not_launched() {
    use super::super::runtime_config::RuntimeConfig;
    let parsed = RuntimeConfig::parse(
        br#"{"agents":{"catalog":"/m.json","workspace":"/w","mcpServers":[
            {"name":"plain","command":"/usr/bin/python3"},
            {"name":"off","command":"/usr/bin/python3","enabled":false,"env":{"TOKEN":"secret"}}
        ]}}"#,
    )
    .unwrap();
    let mut config = parsed.agents.unwrap();
    let rows: Vec<_> = config
        .mcp_servers
        .iter()
        .map(|each| (each.server().name(), each.enabled(), each.env_names()))
        .collect();
    assert_eq!(
        rows,
        [
            ("plain", true, vec![]),
            ("off", false, vec!["TOKEN".to_owned()])
        ]
    );
    assert!(!format!("{config:?}").contains("secret"));
    assert!(RuntimeConfig::parse(
        br#"{"agents":{"catalog":"/m.json","workspace":"/w","mcpServers":[
            {"name":"x","command":"/usr/bin/python3","url":"https://example.com"}]}}"#
    )
    .is_err());
    let namespace = tempfile::tempdir().unwrap();
    let socket = namespace.path().join("mcp").join("relay.sock");
    config.stand_ins = Default::default();
    let composed = compose(&mut config, &socket, Path::new("/nessa"), BTreeMap::new())
        .await
        .unwrap()
        .expect("composed");
    let live: Vec<_> = composed
        .servers
        .configured()
        .into_iter()
        .map(|launch| launch.server.name)
        .collect();
    assert_eq!(live, ["plain"]);
}

/// The composed settings over the real file, lock, parse and audit: a save is
/// published private (0600) as a whole file that the runtime configuration
/// still starts with, never gaining the desktop's managed server; the lock
/// beside it is `config.json.lock`; each change leaves its two records, with
/// variable names and never a value.
#[tokio::test]
async fn composed_settings_publish_privately_under_the_lock_and_audit_without_values() {
    use super::super::runtime_config::RuntimeConfig;
    use super::settings;
    use crate::mcp_servers::application::{McpServerInitiator, McpServerSettingsError};
    use crate::mcp_servers::domain::{ServerEdit, ServerSave, MANAGED_SERVER_NAME};
    use crate::mcp_servers::infrastructure::{ConfigFiles, OsConfigFiles};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    // A namespace is private, as composition creates it.
    let namespace = root.path().join("namespace");
    nessa_local_storage::create_directory(&namespace).unwrap();
    let config_path = namespace.join("config.json");
    nessa_local_storage::open(&config_path, nessa_local_storage::OpenMode::CreateNew)
        .unwrap()
        .write_all(br#"{"session":{"writeTimeoutMs":75}}"#)
        .unwrap();
    // As the desktop composes it: the managed server in memory only.
    let mut config = agents(vec![server(MANAGED_SERVER_NAME, &["--workspace", "/w"])]);
    let composed = compose(
        &mut config,
        &namespace.join("mcp").join("relay.sock"),
        Path::new("/nessa"),
        BTreeMap::new(),
    )
    .await
    .unwrap()
    .expect("composed");
    let audit = namespace.join("audit");
    let settings = settings(&composed, &config, config_path.clone(), audit.clone()).unwrap();
    let initiator = McpServerInitiator {
        organization_id: "organization".into(),
        principal_id: "principal".into(),
        credential_id: "credential".into(),
    };
    let list = settings.list().await.unwrap();
    assert_eq!(list.servers.len(), 1);
    assert!(list.servers[0].managed);
    let save = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: StdioServer::new("mcptest", "/usr/bin/python3", vec!["/s.mjs".into()]),
        env: vec![("API_TOKEN".into(), Some("secret-value".into()))],
        enabled: true,
    });
    // While another holder has `config.json.lock`, nothing is written.
    let holder = OsConfigFiles::new(config_path.clone());
    let held = holder.try_lock().unwrap().expect("the lock");
    assert!(holder.try_lock().unwrap().is_none());
    assert!(namespace.join("config.json.lock").is_file());
    assert_eq!(
        settings
            .edit(initiator.clone(), list.revision.clone(), save.clone())
            .await,
        Err(McpServerSettingsError::Busy)
    );
    drop(held);
    let revision = settings.edit(initiator, list.revision, save).await.unwrap();
    assert_eq!(settings.list().await.unwrap().revision, revision);
    let bytes = std::fs::read(&config_path).unwrap();
    let mode = std::fs::metadata(&config_path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let reread = RuntimeConfig::parse(&bytes).unwrap();
    assert_eq!(
        reread.session().unwrap().write_timeout(),
        std::time::Duration::from_millis(75)
    );
    let agents = reread.agents.unwrap();
    assert_eq!(agents.workspace, std::env::temp_dir());
    let stored: Vec<_> = agents
        .mcp_servers
        .iter()
        .map(|each| each.server().name())
        .collect();
    assert_eq!(stored, ["mcptest"]);
    let live: Vec<_> = composed
        .servers
        .configured()
        .into_iter()
        .map(|launch| launch.server.name)
        .collect();
    assert_eq!(live, ["mcptest", "nessa"]);
    // Two changes, two records each: requested, then the outcome.
    let mut records: Vec<serde_json::Value> = std::fs::read_dir(&audit)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(records.len(), 4);
    records.sort_by_key(|record| record["observedAtMs"].as_u64());
    for record in &records {
        assert_eq!(record["kind"], "mcp_servers");
        assert_eq!(record["target"]["name"], "mcptest");
        assert_eq!(record["cause"], "caller_requested");
        assert_eq!(record["initiator"]["principalId"], "principal");
        assert_eq!(record["initiator"]["credentialId"], "credential");
        assert!(!record.to_string().contains("secret-value"), "{record}");
    }
    let applied = records
        .iter()
        .find(|record| record["transition"]["outcome"] == "applied")
        .expect("an applied outcome");
    assert_eq!(applied["transition"]["after"]["revision"], revision);
    assert_eq!(applied["transition"]["durable"], true);
    assert_eq!(
        applied["transition"]["after"]["names"],
        serde_json::json!(["mcptest"])
    );
    assert_eq!(
        applied["transition"]["before"]["names"],
        serde_json::json!([])
    );
    // The target as stored after, with its variables' names; none before.
    assert_eq!(
        applied["transition"]["after"]["target"],
        serde_json::json!({
            "name": "mcptest",
            "command": "/usr/bin/python3",
            "args": ["/s.mjs"],
            "enabled": true,
            "envNames": ["API_TOKEN"],
        })
    );
    assert_eq!(
        applied["transition"]["before"]["target"],
        serde_json::Value::Null
    );
    let requested = records
        .iter()
        .find(|record| {
            record["phase"] == "requested" && record["operationId"] == applied["operationId"]
        })
        .expect("its requested record");
    // The server asked for: what it would be started with.
    assert_eq!(
        requested["transition"]["server"],
        serde_json::json!({
            "name": "mcptest",
            "command": "/usr/bin/python3",
            "args": ["/s.mjs"],
            "enabled": true,
            "envNames": ["API_TOKEN"],
        })
    );
    // An inspection of the stored server — whose script is not there, so
    // its process ends at once — leaves its two records too, naming the
    // revision that ran and the variables' names.
    let initiator = McpServerInitiator {
        organization_id: "organization".into(),
        principal_id: "principal".into(),
        credential_id: "credential".into(),
    };
    assert!(matches!(
        settings.inspect(initiator, "mcptest").await,
        Err(McpServerSettingsError::Inspect(_))
    ));
    let inspected: Vec<serde_json::Value> = std::fs::read_dir(&audit)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .filter(|record: &serde_json::Value| record["action"] == "inspect")
        .collect();
    assert_eq!(inspected.len(), 2);
    let requested = inspected
        .iter()
        .find(|record| record["phase"] == "requested")
        .expect("its requested record");
    assert_eq!(requested["transition"]["revision"], revision);
    assert_eq!(
        requested["transition"]["server"]["command"],
        "/usr/bin/python3"
    );
    assert_eq!(
        requested["transition"]["server"]["envNames"],
        serde_json::json!(["API_TOKEN"])
    );
    let outcome = inspected
        .iter()
        .find(|record| record["phase"] == "outcome")
        .expect("its outcome");
    assert_eq!(outcome["transition"]["outcome"], "failed");
    assert_eq!(outcome["operationId"], requested["operationId"]);
    for record in &inspected {
        assert_eq!(record["target"]["name"], "mcptest");
        assert_eq!(record["initiator"]["principalId"], "principal");
        assert!(!record.to_string().contains("secret-value"), "{record}");
    }
}

/// A namespace whose `config.json` holds `bytes`, and the MCP composition of
/// a gateway in it, with no server configured.
async fn composed_in(
    bytes: &[u8],
) -> (
    tempfile::TempDir,
    PathBuf,
    super::McpComposition,
    AgentsConfig,
) {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let namespace = root.path().join("namespace");
    nessa_local_storage::create_directory(&namespace).unwrap();
    let config_path = namespace.join("config.json");
    nessa_local_storage::open(&config_path, nessa_local_storage::OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
    let mut config = agents(vec![]);
    let composed = compose(
        &mut config,
        &namespace.join("mcp").join("relay.sock"),
        Path::new("/nessa"),
        BTreeMap::new(),
    )
    .await
    .unwrap()
    .expect("composed");
    (root, config_path, composed, config)
}

fn caller() -> crate::mcp_servers::application::McpServerInitiator {
    crate::mcp_servers::application::McpServerInitiator {
        organization_id: "organization".into(),
        principal_id: "principal".into(),
        credential_id: "credential".into(),
    }
}

fn saved(name: &str, args: Vec<String>) -> crate::mcp_servers::domain::ServerEdit {
    crate::mcp_servers::domain::ServerEdit::Save(crate::mcp_servers::domain::ServerSave {
        previous_name: None,
        server: StdioServer::new(name, "/usr/bin/python3", args),
        env: vec![],
        enabled: true,
    })
}

/// The real file and its lock, with each read counted and the next publish
/// held until it is let go.
struct Held {
    files: crate::mcp_servers::infrastructure::OsConfigFiles,
    reads: std::sync::atomic::AtomicUsize,
    publishing: std::sync::atomic::AtomicBool,
    gate: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}
impl Held {
    fn new(path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            files: crate::mcp_servers::infrastructure::OsConfigFiles::new(path),
            reads: Default::default(),
            publishing: Default::default(),
            gate: Default::default(),
        })
    }
}
impl crate::mcp_servers::infrastructure::ConfigFiles for Held {
    fn read(&self, limit: usize) -> std::io::Result<Option<Vec<u8>>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.files.read(limit)
    }
    fn publish(
        &self,
        bytes: &[u8],
    ) -> std::io::Result<crate::mcp_servers::infrastructure::Published> {
        self.publishing
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        self.files.publish(bytes)
    }
    fn try_lock(&self) -> std::io::Result<Option<crate::mcp_servers::application::StoreLock>> {
        self.files.try_lock()
    }
}

/// Wait, five real seconds at most, until `done`.
async fn within(what: &str, done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "never: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `task`'s answer, within ten real seconds: a test fails rather than hangs.
async fn joined<T>(task: tokio::task::JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("answered in time")
        .unwrap()
}

/// S3 on the real lock: two gateways' settings over one `config.json` — two
/// stores, each with its own `OsConfigFiles` and so its own `flock` — save
/// at one revision at once. Writer A is held between its re-read and its
/// publish; B, waiting on the `flock`, reads nothing until A has published,
/// then is refused with A's revision, and the file holds A's server alone.
/// Without the lock, B would read the old revision in that window and both
/// would publish.
#[tokio::test]
async fn s3_two_saves_at_one_revision_over_the_real_lock_are_serialised() {
    use super::settings_over;
    use crate::mcp_servers::application::McpServerSettingsError;
    use std::sync::atomic::Ordering;
    let (root, config_path, composed, config) =
        composed_in(br#"{"session":{"writeTimeoutMs":75}}"#).await;
    let a_files = Held::new(config_path.clone());
    let b_files = Held::new(config_path.clone());
    let first = Arc::new(
        settings_over(
            &composed,
            &config,
            a_files.clone(),
            root.path().join("audit-1"),
        )
        .unwrap(),
    );
    let second = Arc::new(
        settings_over(
            &composed,
            &config,
            b_files.clone(),
            root.path().join("audit-2"),
        )
        .unwrap(),
    );
    let revision = first.list().await.unwrap().revision;
    let (release, gate) = std::sync::mpsc::channel();
    *a_files.gate.lock().unwrap() = Some(gate);
    let a = tokio::spawn({
        let first = first.clone();
        let revision = revision.clone();
        async move {
            first
                .edit(caller(), revision, saved("a", vec!["/a.py".into()]))
                .await
        }
    });
    within("A reaches its publish", || {
        a_files.publishing.load(Ordering::SeqCst)
    })
    .await;
    let b_reads = b_files.reads.load(Ordering::SeqCst);
    let b = tokio::spawn({
        let second = second.clone();
        let revision = revision.clone();
        async move {
            second
                .edit(caller(), revision, saved("b", vec!["/b.py".into()]))
                .await
        }
    });
    // Real time, well inside the lock's two-second wait.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        b_files.reads.load(Ordering::SeqCst),
        b_reads,
        "B read while A held the flock between its re-read and its publish"
    );
    release.send(()).unwrap();
    let won = joined(a).await.unwrap();
    let lost = joined(b).await.unwrap_err();
    assert_eq!(
        lost,
        McpServerSettingsError::RevisionConflict {
            revision: won.clone()
        }
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    let names: Vec<_> = stored["agents"]["mcpServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["a"]);
    assert_eq!(second.list().await.unwrap().revision, won);
}

/// The configuration's 64 KiB bound (`MAX_CONFIG_BYTES`) at its edge, on the
/// real file and the runtime configuration's own check: a file of exactly
/// 65536 bytes is read and one byte more is `config_too_large`. The bound is
/// on the bytes written: a result exactly 65536 bytes pretty-printed is
/// published so; one a byte longer pretty, but within the bound compact, is
/// published compact; one exactly 65536 bytes compact is published, and one
/// a byte longer refused, the file unchanged. At that edge a remove still
/// publishes, compact, though the rest pretty-printed would not fit.
#[tokio::test]
async fn the_configuration_bound_holds_at_exactly_its_edge() {
    use super::super::runtime_config::MAX_CONFIG_BYTES;
    use super::settings;
    use crate::mcp_servers::application::McpServerSettingsError;
    assert_eq!(MAX_CONFIG_BYTES, 65_536);
    let padded = |size: usize| {
        let head = br#"{"session":{"writeTimeoutMs":75}"#;
        let mut bytes = head.to_vec();
        bytes.resize(size - 1, b' ');
        bytes.push(b'}');
        bytes
    };
    for (size, readable) in [(MAX_CONFIG_BYTES, true), (MAX_CONFIG_BYTES + 1, false)] {
        let (root, config_path, composed, config) = composed_in(&padded(size)).await;
        assert_eq!(
            std::fs::metadata(&config_path).unwrap().len() as usize,
            size
        );
        let settings =
            settings(&composed, &config, config_path, root.path().join("audit")).unwrap();
        let listed = settings.list().await;
        assert_eq!(
            listed.is_ok(),
            readable,
            "{size}: {:?}",
            listed.as_ref().err()
        );
        if !readable {
            assert_eq!(listed, Err(McpServerSettingsError::ConfigTooLarge));
        }
    }
    // A write: measure one save, then make the next land exactly on the
    // edge. Eight long arguments (each at most 8192 bytes) and a last one
    // whose length is the one that moves; and a small server beside it.
    let (root, config_path, composed, config) =
        composed_in(br#"{"session":{"writeTimeoutMs":75}}"#).await;
    let settings = settings(
        &composed,
        &config,
        config_path.clone(),
        root.path().join("audit"),
    )
    .unwrap();
    let size = || std::fs::metadata(&config_path).unwrap().len() as usize;
    let file = || std::fs::read(&config_path).unwrap();
    let compact = |bytes: &[u8]| !bytes[..bytes.len() - 1].contains(&b'\n');
    let args = |last: usize| {
        let mut args = vec!["x".repeat(7600); 8];
        args.push("y".repeat(last));
        args
    };
    let revision = settings.list().await.unwrap().revision;
    let revision = settings
        .edit(caller(), revision, saved("b", vec!["/b.py".into()]))
        .await
        .unwrap();
    let revision = settings
        .edit(caller(), revision, saved("a", args(1000)))
        .await
        .unwrap();
    // Pretty-printed, exactly at the edge.
    let edge = 1000 + MAX_CONFIG_BYTES - size();
    let revision = settings
        .edit(caller(), revision, saved("a", args(edge)))
        .await
        .unwrap();
    assert_eq!(size(), MAX_CONFIG_BYTES);
    assert!(!compact(&file()));
    // A byte past it pretty-printed: written compact, within the bound.
    let revision = settings
        .edit(caller(), revision, saved("a", args(edge + 1)))
        .await
        .unwrap();
    assert!(compact(&file()));
    assert!(size() < MAX_CONFIG_BYTES);
    // Compact, exactly at the edge; then a byte past it is refused.
    let compact_edge = edge + 1 + MAX_CONFIG_BYTES - size();
    let revision = settings
        .edit(caller(), revision, saved("a", args(compact_edge)))
        .await
        .unwrap();
    assert_eq!(size(), MAX_CONFIG_BYTES);
    assert!(compact(&file()));
    let at_edge = file();
    assert_eq!(
        settings
            .edit(
                caller(),
                revision.clone(),
                saved("a", args(compact_edge + 1))
            )
            .await,
        Err(McpServerSettingsError::ConfigTooLarge)
    );
    assert_eq!(file(), at_edge);
    // A remove shrinks it: what is left would not fit pretty-printed, and
    // is written compact.
    let rest: serde_json::Value = serde_json::from_slice(&at_edge).unwrap();
    let mut rest = rest;
    rest["agents"]["mcpServers"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["name"] != "b");
    assert!(serde_json::to_vec_pretty(&rest).unwrap().len() + 1 > MAX_CONFIG_BYTES);
    let removed = crate::mcp_servers::domain::ServerEdit::Remove { name: "b".into() };
    settings.edit(caller(), revision, removed).await.unwrap();
    assert!(compact(&file()));
    assert!(size() < MAX_CONFIG_BYTES);
}

/// Whether `pid` is still running: a zombie is not.
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

/// The process id written to `file`, once written, within five real seconds.
async fn pid_in(file: &Path) -> i64 {
    let read = || {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.parse().ok())
    };
    within("the pid is written", || read().is_some()).await;
    read().unwrap()
}

/// The records in `audit`, in the order they were observed.
fn records_in(audit: &Path) -> Vec<serde_json::Value> {
    let mut records: Vec<serde_json::Value> = std::fs::read_dir(audit)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    records.sort_by_key(|record| record["observedAtMs"].as_u64());
    records
}

/// LS3e, on the composed gateway: shutdown while an inspection is blocked
/// mid-read — a real server that answered `initialize`, is asked for its
/// tools, never answers and ignores its stdin closing. The MCP stop cuts the
/// inspection, writes its outcome (`inspected`, `cut: stopping`, no tools)
/// before it returns, kills the server's process group, and returns well
/// within the drain's bound — not after the inspection's deadline.
#[tokio::test]
async fn shutdown_cuts_an_inspection_blocked_mid_read_and_records_it_before_the_stop() {
    use super::{settings, stop};
    use crate::mcp_servers::application::{InspectCut, Inspection};
    let (root, config_path, composed, config) =
        composed_in(br#"{"session":{"writeTimeoutMs":75}}"#).await;
    let audit = root.path().join("audit");
    let settings = Arc::new(settings(&composed, &config, config_path, audit.clone()).unwrap());
    let pid_file = root.path().join("pid");
    let child_pid_file = root.path().join("child");
    let listed_file = root.path().join("listed");
    let fixture = format!(
        "{}/../nessa-sdk/tests/infrastructure/mcp/fixtures/server.py",
        env!("CARGO_MANIFEST_DIR")
    );
    let arguments = [
        fixture.as_str(),
        "--silent-on-list",
        "--ignore-eof",
        "--child",
        "--pid-file",
        pid_file.to_str().unwrap(),
        "--child-pid-file",
        child_pid_file.to_str().unwrap(),
        "--listed-file",
        listed_file.to_str().unwrap(),
    ];
    let revision = settings.list().await.unwrap().revision;
    settings
        .edit(
            caller(),
            revision,
            saved(
                "fixture",
                arguments.iter().map(|&each| each.into()).collect(),
            ),
        )
        .await
        .unwrap();
    let inspecting = tokio::spawn({
        let settings = settings.clone();
        async move { settings.inspect(caller(), "fixture").await }
    });
    let pid = pid_in(&pid_file).await;
    let child = pid_in(&child_pid_file).await;
    within("the tools are asked for", || listed_file.exists()).await;
    assert!(!inspecting.is_finished());
    let bound = settings.drain_bound();
    let started = std::time::Instant::now();
    assert_eq!(
        tokio::time::timeout(bound, stop(Some(&settings), &composed.servers))
            .await
            .expect("stopped within the drain's bound"),
        Ok(())
    );
    let took = started.elapsed();
    assert!(took < Duration::from_secs(2), "{took:?}");
    // Written before the stop returned: the inspection's two records, after
    // the save's two.
    let records = records_in(&audit);
    assert_eq!(records.len(), 4);
    let outcome = &records[3];
    assert_eq!(outcome["action"], "inspect");
    assert_eq!(outcome["phase"], "outcome");
    assert_eq!(outcome["transition"]["outcome"], "inspected");
    assert_eq!(outcome["transition"]["cut"], "stopping");
    assert_eq!(outcome["transition"]["tools"], 0);
    // Ended by the gateway stopping: the system's, its caller on the
    // requested record.
    assert_eq!(outcome["cause"], "gateway_stopping");
    assert_eq!(outcome["initiator"], serde_json::json!({"kind": "system"}));
    assert_eq!(records[2]["cause"], "caller_requested");
    assert_eq!(records[2]["operationId"], outcome["operationId"]);
    // Killed with its group.
    within("the server and its child are gone", || {
        !alive(pid) && !alive(child)
    })
    .await;
    assert_eq!(
        joined(inspecting).await,
        Ok(Inspection {
            tools: vec![],
            cut: Some(InspectCut::Stopping),
        })
    );
}

/// An inspection that was not started — shutdown before its launch, or the
/// client refusing it — is recorded `failed`, `stopping`, with
/// `started: false` in so many words.
#[test]
fn a_stopping_record_says_the_server_was_not_started() {
    use crate::mcp_servers::application::{
        McpServerAction, McpServerAudit, McpServerAuditPhase, McpServerAuditRecord, McpServerCause,
        McpServerChangeRequest, McpServerOutcome,
    };
    use crate::mcp_servers::infrastructure::DurableMcpServerAudit;
    let root = tempfile::tempdir().unwrap();
    let audit = root.path().join("audit");
    DurableMcpServerAudit::new(
        audit.clone(),
        Arc::new(super::super::local_auth::SystemClock),
    )
    .unwrap()
    .record(&McpServerAuditRecord {
        operation_id: "operation".into(),
        cause: McpServerCause::GatewayStopping,
        request: McpServerChangeRequest {
            action: McpServerAction::Inspect,
            target: "fixture".into(),
            previous_name: None,
            revision: "revision".into(),
            server: None,
        },
        phase: McpServerAuditPhase::Outcome(McpServerOutcome::InspectFailed {
            reason: "stopping",
            started: false,
        }),
    })
    .unwrap();
    let records = records_in(&audit);
    assert_eq!(
        records[0]["transition"],
        serde_json::json!({"outcome": "failed", "reason": "stopping", "started": false})
    );
    // Ended by the gateway stopping, not by its caller.
    assert_eq!(records[0]["cause"], "gateway_stopping");
    assert_eq!(
        records[0]["initiator"],
        serde_json::json!({"kind": "system"})
    );
}

/// C-dup: a variable named twice in a stored server's `env` is refused, in
/// either order — at startup, by the runtime configuration's parse, and on
/// a write, by the store's check of the file it reads — before decoding into
/// a map could keep one value silently.
#[tokio::test]
async fn a_repeated_variable_name_in_the_file_is_refused_in_either_order() {
    use super::super::runtime_config::RuntimeConfig;
    use super::settings;
    use crate::mcp_servers::application::McpServerSettingsError;
    // Both orders, and the same value twice: a repetition is refused for
    // its name, never compared by value.
    for env in [
        r#"{"TOKEN":"first","TOKEN":"second"}"#,
        r#"{"TOKEN":"second","TOKEN":"first"}"#,
        r#"{"TOKEN":"first","TOKEN":"first"}"#,
    ] {
        let file = format!(
            r#"{{"agents":{{"catalog":"/m.json","workspace":"/w","mcpServers":[
                {{"name":"dup","command":"/usr/bin/python3","env":{env}}}]}}}}"#
        );
        let Err(refused) = RuntimeConfig::parse(file.as_bytes()) else {
            panic!("started with {env}")
        };
        let said = format!("{refused:?}");
        assert!(said.contains("TOKEN") && said.contains("dup"), "{said}");
        assert!(
            !said.contains("first") && !said.contains("second"),
            "{said}"
        );
        let (root, config_path, composed, config) = composed_in(file.as_bytes()).await;
        let settings = settings(
            &composed,
            &config,
            config_path.clone(),
            root.path().join("audit"),
        )
        .unwrap();
        assert_eq!(
            settings.list().await,
            Err(McpServerSettingsError::ConfigInvalid)
        );
        assert_eq!(
            settings
                .edit(caller(), "any".into(), saved("other", vec![]))
                .await,
            Err(McpServerSettingsError::ConfigInvalid)
        );
        // Never repaired.
        assert_eq!(std::fs::read(&config_path).unwrap(), file.as_bytes());
    }
}

/// C-null: `"agents": null` is absent to the reader — the runtime
/// configuration starts with no agents block — and to the writer, which
/// lists no stored server and writes a block from the running catalog and
/// workspace that the runtime configuration then starts with.
#[tokio::test]
async fn c_null_agents_is_read_and_written_as_absent() {
    use super::super::runtime_config::RuntimeConfig;
    use super::settings;
    let file = br#"{"session":{"writeTimeoutMs":75},"agents":null}"#;
    assert!(RuntimeConfig::parse(file).unwrap().agents.is_none());
    let (root, config_path, composed, config) = composed_in(file).await;
    let settings = settings(
        &composed,
        &config,
        config_path.clone(),
        root.path().join("audit"),
    )
    .unwrap();
    let list = settings.list().await.unwrap();
    assert!(list.servers.is_empty());
    settings
        .edit(caller(), list.revision, saved("mcptest", vec![]))
        .await
        .unwrap();
    let agents = RuntimeConfig::parse(&std::fs::read(&config_path).unwrap())
        .unwrap()
        .agents
        .unwrap();
    assert_eq!(agents.catalog, config.catalog);
    assert_eq!(agents.workspace, config.workspace);
    let stored: Vec<_> = agents
        .mcp_servers
        .iter()
        .map(|each| each.server().name())
        .collect();
    assert_eq!(stored, ["mcptest"]);
}

/// `config.json.lock` planted as a FIFO is refused, not waited on: opened
/// without blocking, so `open` returns at once with no reader on the other
/// end, and refused as not a regular file when one is there.
#[test]
fn a_lock_that_is_not_a_regular_file_is_refused_without_blocking() {
    use crate::mcp_servers::infrastructure::{ConfigFiles, OsConfigFiles};
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let root = tempfile::tempdir().unwrap();
    let config_path = root.path().join("config.json");
    let lock_path = root.path().join("config.json.lock");
    let fifo = std::ffi::CString::new(lock_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: mkfifo reads the NUL-terminated path only.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    // On a thread of its own, so a lock that blocks fails the test rather
    // than hanging it.
    let try_lock = |path: PathBuf| {
        let (answer, answered) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(
                OsConfigFiles::new(path)
                    .try_lock()
                    .map(|lock| lock.is_some()),
            );
        });
        answered
            .recv_timeout(Duration::from_secs(5))
            .expect("answered without blocking")
    };
    // No reader: the open itself is refused.
    assert!(try_lock(config_path.clone()).is_err());
    // A reader: opened, then refused as not a regular file — before any
    // `flock`, which some systems would take on a FIFO.
    let _reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&lock_path)
        .unwrap();
    assert_eq!(
        try_lock(config_path).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}
