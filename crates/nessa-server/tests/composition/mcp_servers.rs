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

#[cfg(unix)]
#[test]
fn a_gateway_path_that_is_not_utf8_has_no_stand_ins() {
    use std::os::unix::ffi::OsStrExt;
    let gateway = PathBuf::from(std::ffi::OsStr::from_bytes(b"/app/\xff/nessa"));
    assert_eq!(
        stand_ins(&[server("s", &[])], &gateway, Path::new("/tmp/s.sock")),
        None
    );
}

#[cfg(unix)]
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

#[cfg(unix)]
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
    // A second run replaces the socket the first left behind.
    drop(composed);
    let mut again = agents(vec![configured]);
    assert!(
        compose(&mut again, &socket, Path::new("/nessa"), BTreeMap::new())
            .await
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_relay_that_cannot_be_bound_leaves_mcp_servers_off() {
    let namespace = tempfile::tempdir().unwrap();
    // Something that is not a socket where the socket goes is left alone.
    let socket = namespace.path().join("mcp").join("relay.sock");
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    std::fs::write(&socket, b"not a socket").unwrap();
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
