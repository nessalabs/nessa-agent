//! The process-global tracing subscriber, and the one thing it decides:
//! whether its output is for a screen or for a file.
//!
//! The packaged gateway is started by launchd, which redirects this process's
//! stderr straight into `~/.nessa/logs/gateway.log`. Colour written there is
//! not colour, it is `\u{1b}[2m` and `\u{1b}[31m` wrapped around every field of
//! the one artefact a person can send us — harder to read, and harder to grep
//! for the line that matters. Colour is for a terminal, so it is asked for only
//! when stderr is one.
use std::io::IsTerminal;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Every Nessa crate the gateway is built from has its own directive here: a
/// filter made of named targets drops every target it does not name, errors
/// included, so a crate split out of the gateway would otherwise go silent
/// (`every_nessa_crate_the_gateway_is_built_from_has_a_directive`).
const DEFAULT_FILTER: &str = concat!(
    // The gateway, and the crates split out of it: their lines are its lines.
    "nessa_server=info,nessa_conversation=info,nessa_agent_install=info,",
    "nessa_attachments=info,nessa_backup=info,nessa_device_pairing=info,",
    "nessa_env_serve=info,nessa_mcp_authorization=info,nessa_peer_gateways=info,",
    "nessa_webhooks=info,",
    "nessa_sdk=warn,nessa_sdk::timing=info,nessa_gateway_endpoint=error,",
    // Libraries it uses, quiet unless RUST_LOG asks for them.
    "nessa_agent_credentials=off,nessa_auth=off,nessa_client_core=off,",
    "nessa_device_client=off,nessa_images=off,nessa_local_database=off,",
    "nessa_local_storage=off,nessa_projection=off,nessa_protocol=off,",
    "nessa_timer=off,nessa_vault=off",
);

/// Install the process-global tracing subscriber. Call once at startup, before any log line.
pub fn init() {
    subscriber(std::io::stderr, std::io::stderr().is_terminal(), filter()).init();
}

/// What is logged when the environment names nothing.
///
/// The gateway's own crates log at info. The SDK is where an agent actually runs, and the endpoint crate owns the
/// publication steps required before the gateway can announce that it is
/// listening. Their warnings and errors are the operator's to see. Info and
/// debug lines stay off, except the bounded startup and first-response timings.
fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter())
}

fn default_filter() -> EnvFilter {
    EnvFilter::new(DEFAULT_FILTER)
}

/// The subscriber `init` installs, over any writer, so what it writes can be
/// read back in a test instead of guessed at.
fn subscriber<W>(
    writer: W,
    colour: bool,
    filter: EnvFilter,
) -> impl tracing::Subscriber + Send + Sync + 'static
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(colour)
        .with_writer(writer)
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// A writer the test can read back, shared with the subscriber.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl Captured {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured log").clone()).expect("utf-8 log")
        }
    }
    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("captured log").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn written(colour: bool) -> String {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing::subscriber::with_default(
            subscriber(
                move || writer.clone(),
                colour,
                EnvFilter::new("nessa_server=info"),
            ),
            || tracing::error!(detail = "registry is invalid", "nessa failed"),
        );
        captured.text()
    }

    /// The gateway's log is a file launchd opened, and a file is what this
    /// process's stderr is whenever it is not a terminal.
    #[test]
    fn a_log_that_is_not_a_terminal_has_no_escape_sequences_in_it() {
        let file = written(false);
        assert!(file.contains("nessa failed"), "{file}");
        assert!(file.contains("registry is invalid"), "{file}");
        assert!(!file.contains('\u{1b}'), "{file:?}");
        // A terminal still gets its colour: this is one decision, not the
        // removal of a feature.
        assert!(written(true).contains('\u{1b}'));
    }

    /// A Nessa crate this one depends on, with no directive of its own,
    /// would have its errors dropped by the default filter.
    #[test]
    fn every_nessa_crate_the_gateway_is_built_from_has_a_directive() {
        let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        let named: Vec<&str> = DEFAULT_FILTER
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
        assert!(crates.len() > 10, "the manifest's Nessa crates: {crates:?}");
        for name in crates {
            assert!(named.contains(&name.as_str()), "no directive for {name}");
        }
    }

    /// A crate split out of the gateway keeps the gateway's logs: an error the
    /// conversation crate logs is written.
    #[test]
    fn the_default_filter_writes_a_split_crates_errors() {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing::subscriber::with_default(
            subscriber(move || writer.clone(), false, default_filter()),
            || {
                tracing::error!(target: "nessa_conversation::application", "a turn failed");
                tracing::info!(target: "nessa_webhooks::infrastructure", "a delivery started");
                tracing::info!(target: "nessa_auth::adapters", "a credential was read");
            },
        );
        let log = captured.text();
        assert!(log.contains("a turn failed"), "{log}");
        assert!(log.contains("a delivery started"), "{log}");
        assert!(!log.contains("a credential was read"), "{log}");
    }

    #[test]
    fn the_default_filter_keeps_endpoint_publication_phase_diagnostics() {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing::subscriber::with_default(
            subscriber(move || writer.clone(), false, default_filter()),
            || {
                tracing::error!(
                    target: "nessa_gateway_endpoint::infrastructure::file",
                    operation = "replace endpoint record",
                    error.kind = "PermissionDenied",
                    error.raw_os_code = 5,
                    "gateway endpoint publication failed",
                );
            },
        );

        let log = captured.text();
        assert!(
            log.contains("nessa_gateway_endpoint::infrastructure::file"),
            "{log}"
        );
        assert!(log.contains("gateway endpoint publication failed"), "{log}");
        assert!(
            log.contains("operation=\"replace endpoint record\""),
            "{log}"
        );
        assert!(log.contains("error.raw_os_code=5"), "{log}");
    }
    #[test]
    fn default_filter_includes_timings_without_enabling_other_sdk_info() {
        let captured = Captured::default();
        let writer = captured.clone();
        tracing::subscriber::with_default(
            subscriber(move || writer.clone(), false, default_filter()),
            || {
                tracing::info!(target: "nessa_sdk::timing", metric = "agent_first_text_response_ms", elapsed_ms = 50.0, "agent first text response");
                tracing::info!(target: "nessa_sdk::other", "unrelated sdk detail");
            },
        );
        let log = captured.text();
        assert!(log.contains("agent_first_text_response_ms"), "{log}");
        assert!(!log.contains("unrelated sdk detail"), "{log}");
    }
}
