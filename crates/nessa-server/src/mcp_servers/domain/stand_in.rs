//! What a harness is given in place of a configured MCP server, and whether
//! the gateway lets a stand-in through.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

/// The `nessa` subcommand a stand-in runs.
pub const RELAY_SUBCOMMAND: &str = "mcp-relay";

/// Why the gateway turned a stand-in away. Each is said to the relay, which
/// exits non-zero, so the harness sees its server fail to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandInRefusal {
    /// Its session token was never issued, or its grant has been revoked: it
    /// belongs to no open conversation.
    UnknownSession,
    /// No server is configured under the name it gave.
    UnknownServer,
    /// The server under that name is configured differently now than when the
    /// stand-in was made.
    ConfigurationChanged,
    /// The server could not be started.
    Unavailable,
}

/// A digest of what a configured server is started as: its `command` and its
/// `args`, each length-prefixed so no two configurations share one. Its name
/// is not in it; the stand-in carries that beside it.
pub fn configuration_digest(command: &Path, args: &[String]) -> String {
    let mut hash = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    };
    field(command.as_os_str().as_encoded_bytes());
    field(&(args.len() as u64).to_be_bytes());
    for arg in args {
        field(arg.as_bytes());
    }
    format!("sha256:{:x}", hash.finalize())
}

/// The arguments a stand-in for `server` runs with: [`RELAY_SUBCOMMAND`], the
/// relay `socket`, the server's name, and its [`configuration_digest`]. Every
/// one is stable across runs of one namespace, and the digest changes when the
/// configured server does — which is what the relay compares, refusing a
/// stand-in whose server changed with `configuration-changed`. None of it is
/// part of a conversation's restoration identity (ADR 344).
pub fn relay_arguments(socket: &str, server: &str, configuration: &str) -> Vec<String> {
    vec![
        RELAY_SUBCOMMAND.into(),
        socket.into(),
        server.into(),
        configuration.into(),
    ]
}

/// Whether a stand-in naming `server` with `configuration` is let through,
/// given each configured server's digest by name.
pub fn admit(
    server: &str,
    configuration: &str,
    configured: &BTreeMap<String, String>,
) -> Result<(), StandInRefusal> {
    match configured.get(server) {
        None => Err(StandInRefusal::UnknownServer),
        Some(current) if current != configuration => Err(StandInRefusal::ConfigurationChanged),
        Some(_) => Ok(()),
    }
}
