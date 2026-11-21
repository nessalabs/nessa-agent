//! Nessa gateway with mandatory credential authentication and per-operation authorization.
//!
//! This crate is composition only: it reads configuration, wires the gateway
//! (`nessa-gateway`) to its stores and features, and starts it. The gateway's
//! modules are re-exported under their old paths.
// The one clock: outside tests, time is read only through crate::timer::clock
// (clippy.toml names the calls it replaces; CODING_STANDARDS.md says why).
#![cfg_attr(not(test), warn(clippy::disallowed_methods))]
mod bootstrap;
pub mod composition;
pub use nessa_conversation as conversation;
#[cfg(unix)]
pub use nessa_gateway::workspaces;
pub use nessa_gateway::{
    agent_warm_up, agents, app, attachments, backup, browser_session, cli, core, desktop_runtime,
    device_pairing, devices, env, external_events, github, github_relay, health, limits, logins,
    mcp_servers, product, relay, secrets, server, terminal, timer, vault, voice, webhooks,
};

pub use bootstrap::run;

#[cfg(test)]
pub(crate) use nessa_conversation::conversation_test_support;

#[cfg(all(test, unix))]
pub(crate) use nessa_conversation::app_call_test_support;

#[cfg(test)]
mod logging_tests {
    /// The `nessa` binary is this crate: every Nessa crate it is built from,
    /// and its own composition lines, need a directive in the gateway's
    /// default filter, or a filter of named targets drops them, errors
    /// included.
    #[test]
    fn every_nessa_crate_the_binary_is_built_from_has_a_directive() {
        let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        let named: Vec<&str> = crate::core::logging::DEFAULT_FILTER
            .split(',')
            .map(|directive| directive.split('=').next().unwrap_or_default())
            .collect();
        let mut crates: Vec<String> = manifest
            .lines()
            .filter(|line| line.starts_with("nessa-") && line.contains("path = \"../"))
            .filter_map(|line| line.split_whitespace().next())
            .map(|name| name.replace('-', "_"))
            .collect();
        crates.push(env!("CARGO_CRATE_NAME").to_owned());
        assert!(crates.contains(&"nessa_gateway".to_owned()), "{crates:?}");
        for name in crates {
            assert!(named.contains(&name.as_str()), "no directive for {name}");
        }
    }
}
