//! The process-global tracing subscriber, and the one thing it decides:
//! whether its output is for a screen or for a file.
//!
//! The packaged gateway is started by launchd, which redirects this process's
//! stderr straight into `~/.nessa/logs/gateway.log`. Colour written there is
//! not colour, it is `\u{1b}[2m` and `\u{1b}[31m` wrapped around every field of
//! the one artefact a person can send us — harder to read, and harder to grep
//! for the line that matters. Colour is for a terminal, so it is asked for only
//! when stderr is one.
//!
//! It also sends each decision's trace (`nessa_conversation::DECISION_TARGET`)
//! to an OpenTelemetry collector, when the standard
//! `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`
//! names one. Nothing else is sent, and with neither set nothing is.
use nessa_conversation::application::DECISION_TARGET;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::io::IsTerminal;
use std::sync::OnceLock;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// The trace pipeline `init` started, kept so `shutdown` can send what it holds.
static TRACES: OnceLock<SdkTracerProvider> = OnceLock::new();

/// Every Nessa crate the gateway is built from has its own directive here: a
/// filter made of named targets drops every target it does not name, errors
/// included, so a crate split out of the gateway would otherwise go silent
/// (`every_nessa_crate_the_gateway_is_built_from_has_a_directive`).
pub const DEFAULT_FILTER: &str = concat!(
    // The gateway, and the crates split out of it: their lines are its lines.
    "nessa_server=info,nessa_gateway=info,nessa_conversation=info,",
    "nessa_agent_install=info,",
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
    let traces = collector_configured(|name| std::env::var_os(name).is_some())
        .then(|| decision_traces(None))
        .flatten()
        .map(|(provider, layer)| {
            let _ = TRACES.set(provider);
            layer
        });
    subscriber(std::io::stderr, std::io::stderr().is_terminal(), filter())
        .with(traces)
        .init();
}

/// Send the decision traces still held, before the process exits. Bounded by
/// the exporter's own timeout; nothing to do when none were configured.
pub fn shutdown() {
    if let Some(provider) = TRACES.get() {
        if let Err(error) = provider.shutdown() {
            tracing::warn!(%error, "decision traces could not all be sent");
        }
    }
}

/// Whether the environment names a collector, by the variables every
/// OpenTelemetry exporter reads.
fn collector_configured(set: impl Fn(&str) -> bool) -> bool {
    [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    ]
    .into_iter()
    .any(set)
}

/// The layer that sends decision spans, and only those, over OTLP/HTTP to
/// the collector the environment names (or `endpoint`, a full traces URL),
/// with the pipeline that sends them. A collector that cannot be set up is
/// said once and left out: the gateway runs the same without it.
fn decision_traces<S>(endpoint: Option<&str>) -> Option<(SdkTracerProvider, impl Layer<S>)>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    use opentelemetry_otlp::WithExportConfig;
    // The exporter's HTTP client is the gateway's reqwest, built without a
    // TLS provider of its own: the process's is ring, as for every other client.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let builder = opentelemetry_otlp::SpanExporter::builder().with_http();
    let builder = match endpoint {
        Some(endpoint) => builder.with_endpoint(endpoint),
        None => builder,
    };
    let exporter = match builder.build() {
        Ok(exporter) => exporter,
        Err(error) => {
            eprintln!("nessa: decision traces are off: the OTLP exporter could not start: {error}");
            return None;
        }
    };
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name("nessa-gateway")
                .build(),
        )
        .build();
    let tracer = provider.tracer("nessa-gateway");
    Some((
        provider,
        tracing_opentelemetry::layer()
            .with_tracer(tracer)
            .with_filter(decision_spans()),
    ))
}

/// Decision spans and the events inside them; nothing else leaves the process.
fn decision_spans() -> Targets {
    Targets::new().with_target(DECISION_TARGET, tracing::Level::INFO)
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
) -> impl tracing::Subscriber
       + for<'span> tracing_subscriber::registry::LookupSpan<'span>
       + Send
       + Sync
       + 'static
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    Registry::default().with(
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(colour)
            .with_writer(writer)
            .with_filter(filter),
    )
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
                EnvFilter::new(concat!(env!("CARGO_CRATE_NAME"), "=info")),
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
    /// Traces leave the process only when a collector is named, and then
    /// only decisions: no other span or line is sent.
    #[test]
    fn only_decisions_are_traced_and_only_to_a_named_collector() {
        assert!(!collector_configured(|_| false));
        assert!(collector_configured(
            |name| name == "OTEL_EXPORTER_OTLP_ENDPOINT"
        ));
        assert!(collector_configured(
            |name| name == "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"
        ));
        let spans = decision_spans();
        let meta = |target: &'static str| spans.would_enable(target, &tracing::Level::INFO);
        assert!(meta(DECISION_TARGET));
        assert!(!meta("nessa_conversation::application"));
        assert!(!meta("nessa_gateway::product"));
        assert!(!spans.would_enable(DECISION_TARGET, &tracing::Level::DEBUG));
    }

    /// A decision span reaches the collector as OTLP, with its attributes;
    /// a span on any other target does not.
    #[test]
    fn a_decision_span_is_sent_to_the_collector() {
        use std::io::{BufRead, BufReader, Read};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            let mut path = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if path.is_empty() {
                    path = line.clone();
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let mut stream = stream;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
            (path, body)
        });
        let (provider, layer) =
            decision_traces(Some(&format!("http://{address}/v1/traces"))).unwrap();
        tracing::subscriber::with_default(Registry::default().with(layer), || {
            let other = tracing::info_span!(target: "nessa_gateway::product", "not_a_decision");
            other.in_scope(|| {});
            let span = tracing::info_span!(
                target: DECISION_TARGET,
                "decision",
                "decision.outcome" = "auto_picked"
            );
            span.in_scope(|| tracing::info!(target: DECISION_TARGET, "settled"));
        });
        provider.force_flush().unwrap();
        let (path, body) = received.join().unwrap();
        provider.shutdown().unwrap();
        assert!(path.starts_with("POST /v1/traces "), "{path}");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("decision.outcome"), "{body}");
        assert!(body.contains("auto_picked"), "{body}");
        assert!(body.contains("nessa-gateway"), "{body}");
        assert!(!body.contains("not_a_decision"), "{body}");
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
