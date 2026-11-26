//! Telemetry as `config.json` turns it on: which sinks the one
//! [`Telemetry`] dispatcher sends to, started once per process.
//!
//! ```text
//! config.json "telemetry": { "otel": { "enabled": true, "endpoint"? } }
//!        │ absent, or enabled false: Telemetry::off(), no thread, no queue
//!        ▼
//! start ──▶ Telemetry ── conversation service (turn submitted, decisions)
//!                    └── each agent's ObservedExecutionAudit (admitted … ended)
//! process exit ──▶ flush_at_exit: what is queued is sent, bounded
//! ```
//!
//! Only `nessa-gateway`'s spans leave through it. With `otel` on and no
//! `endpoint`, the exporter reads the standard `OTEL_EXPORTER_OTLP_*`
//! variables; those variables alone turn nothing on.
use crate::core::RunError;
use nessa_telemetry::{OtelSettings, OtelSink, Sink, Telemetry, DEFAULT_CAPACITY};
use serde::Deserialize;
use std::{sync::OnceLock, time::Duration};

/// The `telemetry` section of `config.json`. Absent is off.
#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct TelemetryConfig {
    /// Spans over OTLP/HTTP.
    pub otel: Option<OtelConfig>,
}

/// The OpenTelemetry sink's settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct OtelConfig {
    pub enabled: bool,
    /// The full traces URL, `http://` or `https://`, such as
    /// `http://localhost:4318/v1/traces`.
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl TelemetryConfig {
    /// The OTLP settings this section asks for, when it turns OTel on.
    ///
    /// # Errors
    /// [`RunError::RuntimeConfig`] for an endpoint that is not an HTTP URL.
    pub(super) fn otel(&self) -> Result<Option<OtelSettings>, RunError> {
        let Some(otel) = self.otel.as_ref().filter(|otel| otel.enabled) else {
            return Ok(None);
        };
        if let Some(endpoint) = &otel.endpoint {
            let url = reqwest::Url::parse(endpoint).map_err(|_| {
                RunError::RuntimeConfig("telemetry.otel.endpoint must be a URL".into())
            })?;
            if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
                return Err(RunError::RuntimeConfig(
                    "telemetry.otel.endpoint must be an http:// or https:// URL".into(),
                ));
            }
        }
        Ok(Some(OtelSettings {
            endpoint: otel.endpoint.clone(),
            service: SERVICE.into(),
        }))
    }
}

/// `service.name` on every span this process sends.
const SERVICE: &str = "nessa-gateway";

/// The process's dispatcher, once started: the gateway restarts itself in
/// place with `execve`, which starts a new process, so once is per run.
static STARTED: OnceLock<Telemetry> = OnceLock::new();

/// The process's telemetry, started from `config` the first time. A sink
/// that cannot start is said once and left out: the gateway runs the same
/// without it.
///
/// # Errors
/// The section is malformed ([`TelemetryConfig::otel`]).
pub(super) fn start(config: &TelemetryConfig) -> Result<Telemetry, RunError> {
    let otel = config.otel()?;
    if let Some(started) = STARTED.get() {
        return Ok(started.clone());
    }
    let mut sinks: Vec<Box<dyn Sink>> = Vec::new();
    if let Some(settings) = otel {
        match OtelSink::new(&settings) {
            Ok(sink) => sinks.push(Box::new(sink)),
            Err(error) => tracing::warn!(%error, "OpenTelemetry export is off this run"),
        }
    }
    let telemetry = Telemetry::new(sinks, DEFAULT_CAPACITY).unwrap_or_else(|error| {
        tracing::warn!(%error, "telemetry is off this run: its worker could not start");
        Telemetry::off()
    });
    if telemetry.is_on() {
        tracing::info!("telemetry is on: OpenTelemetry spans are exported");
    }
    Ok(STARTED.get_or_init(|| telemetry).clone())
}

/// Send what telemetry still holds before the process exits, waiting at
/// most a few seconds. Nothing to do when it never started or is off.
pub(crate) fn flush_at_exit() {
    let Some(telemetry) = STARTED.get() else {
        return;
    };
    if !telemetry.shutdown(Duration::from_secs(5)) {
        tracing::warn!("telemetry could not all be sent before exit");
    }
    let dropped = telemetry.dropped();
    if dropped > 0 {
        tracing::warn!(dropped, "telemetry events were dropped: the queue was full");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(json: &str) -> TelemetryConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn telemetry_is_off_unless_a_sink_is_enabled() {
        assert!(parsed("{}").otel().unwrap().is_none());
        assert!(
            parsed(r#"{"otel": {"enabled": false, "endpoint": "http://h/v1/traces"}}"#)
                .otel()
                .unwrap()
                .is_none()
        );
        let on = parsed(r#"{"otel": {"enabled": true}}"#)
            .otel()
            .unwrap()
            .unwrap();
        assert_eq!(on.endpoint, None);
        assert_eq!(on.service, "nessa-gateway");
        let named =
            parsed(r#"{"otel": {"enabled": true, "endpoint": "https://c:4318/v1/traces"}}"#)
                .otel()
                .unwrap()
                .unwrap();
        assert_eq!(named.endpoint.as_deref(), Some("https://c:4318/v1/traces"));
    }

    #[test]
    fn an_endpoint_that_is_not_http_is_refused_and_unknown_fields_too() {
        for endpoint in ["file:///tmp/x", "not a url", "unix:/run/collector"] {
            let config = TelemetryConfig {
                otel: Some(OtelConfig {
                    enabled: true,
                    endpoint: Some(endpoint.into()),
                }),
            };
            assert!(
                matches!(config.otel(), Err(RunError::RuntimeConfig(_))),
                "{endpoint}"
            );
        }
        assert!(serde_json::from_str::<TelemetryConfig>(r#"{"posthog": {}}"#).is_err());
        assert!(serde_json::from_str::<TelemetryConfig>(
            r#"{"otel": {"enabled": true, "key": 1}}"#
        )
        .is_err());
    }
}
