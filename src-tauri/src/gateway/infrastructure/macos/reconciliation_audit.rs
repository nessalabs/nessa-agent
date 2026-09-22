//! Private immutable audit records for gateway reconciliation intent, outcome,
//! and callers that join an existing attempt.
//!
//! ```text
//! application audit port -> atomic JSON record -> stage-scoped config root
//! ```
//! Arrows mean durable writes. The adapter never decides reconciliation state.
use super::control::atomic_write;
use crate::gateway::{
    application::{
        GatewayError, GatewayNativeEffect, GatewayReconciliationAttempt,
        GatewayReconciliationAudit, GatewayReconciliationEffect, GatewayReconciliationIntent,
        GatewayReconciliationOutcome, GatewayReconciliationRequest,
    },
    domain::value_objects::{
        BundledSurface, ReconciliationCause, ReconciliationIncarnation, ReconciliationInitiator,
        ReconciliationTarget,
    },
};
use serde_json::{json, Value};
use std::path::PathBuf;

pub(super) struct FileReconciliationAudit {
    directory: Option<PathBuf>,
}

impl FileReconciliationAudit {
    pub(super) fn new(config_root: Option<PathBuf>) -> Self {
        Self {
            directory: config_root.map(|root| root.join("gateway-reconciliation-audit")),
        }
    }

    fn directory(&self) -> Result<PathBuf, GatewayError> {
        let directory = self.directory.clone().ok_or_else(|| {
            GatewayError::Registration(
                "Application config storage is required for gateway audit".into(),
            )
        })?;
        nessa_local_storage::create_directory(&directory)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        Ok(directory)
    }

    fn write(&self, name: String, value: Value) -> Result<(), GatewayError> {
        let directory = self.directory()?;
        let path = directory.join(name);
        if path.exists() {
            return Err(GatewayError::Registration(
                "Gateway reconciliation audit record already exists".into(),
            ));
        }
        let bytes = serde_json::to_vec(&value)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        atomic_write(&path, &bytes).map_err(|error| {
            GatewayError::Registration(format!("Could not record gateway reconciliation: {error}"))
        })
    }
}

impl GatewayReconciliationAudit for FileReconciliationAudit {
    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        self.write(
            format!("{}-intent.json", intent.attempt().correlation().as_str()),
            json!({
                "attemptCorrelation": intent.attempt().correlation().as_str(),
                "origin": request(intent.attempt().origin()),
                "target": target(intent.target()),
                "before": intent.before().map(identity),
            }),
        )
    }

    fn outcome(&self, outcome: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        let effect = match outcome.effect() {
            GatewayReconciliationEffect::Confirmed(after) => {
                json!({"kind":"confirmed", "after":identity(after)})
            }
            GatewayReconciliationEffect::Refused(error) => {
                json!({"kind":"refused", "error":error.to_string()})
            }
            GatewayReconciliationEffect::Partial { effects, error } => {
                json!({
                    "kind":"partial",
                    "effects":effects.iter().map(|effect| native_effect(*effect)).collect::<Vec<_>>(),
                    "error":error.to_string()
                })
            }
        };
        self.write(
            format!("{}-outcome.json", outcome.attempt().correlation().as_str()),
            json!({
                "attemptCorrelation":outcome.attempt().correlation().as_str(),
                "origin":request(outcome.attempt().origin()),
                "target":target(outcome.intent().target()),
                "before":outcome.intent().before().map(identity),
                "effect":effect
            }),
        )
    }

    fn joined(
        &self,
        attempt: &GatewayReconciliationAttempt,
        joined: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        self.write(
            format!("{}-joined.json", joined.correlation().as_str()),
            json!({
                "attemptCorrelation": attempt.correlation().as_str(),
                "originRequestCorrelation": attempt.origin().correlation().as_str(),
                "joinedRequest": request(joined),
            }),
        )
    }
}

fn request(request: &GatewayReconciliationRequest) -> Value {
    json!({
        "correlation": request.correlation().as_str(),
        "cause": cause(request.evidence().cause()),
        "initiator": initiator(request.evidence().initiator()),
    })
}

fn cause(cause: ReconciliationCause) -> &'static str {
    match cause {
        ReconciliationCause::Startup => "startup",
        ReconciliationCause::CredentialLoad => "credential_load",
        ReconciliationCause::ExplicitRetry => "explicit_retry",
        ReconciliationCause::ClaudeConfigurationChanged => "claude_configuration_changed",
    }
}

fn initiator(initiator: ReconciliationInitiator) -> &'static str {
    match initiator {
        ReconciliationInitiator::DesktopHost => "desktop_host",
        ReconciliationInitiator::BundledSurface(BundledSurface::Main) => "main_window",
        ReconciliationInitiator::BundledSurface(BundledSurface::Setup) => "setup_window",
    }
}

fn native_effect(effect: GatewayNativeEffect) -> &'static str {
    match effect {
        GatewayNativeEffect::OldServiceUnloaded => "old_service_unloaded",
        GatewayNativeEffect::ServiceDefinitionPublished => "service_definition_published",
        GatewayNativeEffect::BootstrapRequested => "bootstrap_requested",
    }
}

fn target(target: &ReconciliationTarget) -> Value {
    json!({
        "service": target.service(),
        "runtimeFingerprint": target.runtime_fingerprint(),
        "serviceGeneration": target.service_generation(),
    })
}

fn identity(gateway: &ReconciliationIncarnation) -> Value {
    json!({
        "service": gateway.target().service(),
        "runtimeFingerprint": gateway.target().runtime_fingerprint(),
        "runtimeInstance": gateway.runtime_instance(),
        "serviceGeneration": gateway.target().service_generation(),
        "processId": gateway.process_id(),
        "port": gateway.port(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        application::{GatewayReconciliationAttempt, GatewayReconciliationRequest},
        domain::value_objects::{
            ReconciliationCorrelation, ReconciliationEvidence, ReconciliationTarget,
        },
    };
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn correlation(value: u64) -> ReconciliationCorrelation {
        ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{value:012x}"))
            .expect("correlation")
    }

    fn request(value: u64, cause: ReconciliationCause) -> GatewayReconciliationRequest {
        GatewayReconciliationRequest::new(
            correlation(value),
            ReconciliationEvidence::new(
                cause,
                if cause == ReconciliationCause::Startup {
                    ReconciliationInitiator::DesktopHost
                } else {
                    ReconciliationInitiator::BundledSurface(BundledSurface::Main)
                },
            )
            .expect("evidence"),
        )
    }

    #[test]
    fn records_intent_outcome_and_join_without_overwriting() {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "nessa-reconciliation-audit-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let audit = FileReconciliationAudit::new(Some(root.clone()));
        let attempt = GatewayReconciliationAttempt::new(
            correlation(2),
            request(1, ReconciliationCause::Startup),
        )
        .expect("attempt");
        let target = ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("target");
        let intent =
            GatewayReconciliationIntent::new(attempt.clone(), target, None).expect("intent");
        audit.intent(&intent).expect("intent write");
        assert!(audit.intent(&intent).is_err());

        let outcome = GatewayReconciliationOutcome::new(
            intent,
            GatewayReconciliationEffect::Refused(GatewayError::Registration("refused".into())),
        )
        .expect("outcome");
        audit.outcome(&outcome).expect("outcome write");
        let joined = request(3, ReconciliationCause::ExplicitRetry);
        audit.joined(&attempt, &joined).expect("join write");

        let directory = root.join("gateway-reconciliation-audit");
        let recorded: Value = serde_json::from_slice(
            &fs::read(directory.join("00000000-0000-4000-8000-000000000002-outcome.json"))
                .expect("record"),
        )
        .expect("json");
        assert_eq!(recorded["target"]["runtimeFingerprint"], "a".repeat(64));
        assert_eq!(recorded["origin"]["cause"], "startup");
        assert_eq!(recorded["effect"]["kind"], "refused");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
