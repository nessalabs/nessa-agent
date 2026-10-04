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
            .map(|server| ConfiguredMcpServer {
                server: StdioServer {
                    name: server.name,
                    command: server.command,
                    args: server.args,
                },
                enabled: true,
                env: BTreeMap::new(),
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
        .map(|each| (each.server.name.as_str(), each.enabled, each.env_names()))
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
        server: StdioServer {
            name: "mcptest".into(),
            command: "/usr/bin/python3".into(),
            args: vec!["/s.mjs".into()],
        },
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
        .map(|each| each.server.name.as_str())
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
    assert_eq!(
        applied["transition"]["after"]["names"],
        serde_json::json!(["mcptest"])
    );
    assert_eq!(
        applied["transition"]["before"]["names"],
        serde_json::json!([])
    );
    let requested = records
        .iter()
        .find(|record| {
            record["phase"] == "requested" && record["operationId"] == applied["operationId"]
        })
        .expect("its requested record");
    assert_eq!(
        requested["transition"]["envNames"],
        serde_json::json!(["API_TOKEN"])
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
        requested["transition"]["envNames"],
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
