//! Private immutable audit records for gateway reconciliation intent, outcome,
//! and callers that join an existing attempt.
//!
//! ```text
//! application audit port -> atomic JSON record -> stage-scoped config root
//! ```
//! Arrows mean durable writes. The adapter never decides reconciliation state.
use crate::gateway::{
    application::{
        GatewayError, GatewayReconciliationAttempt, GatewayReconciliationAudit,
        GatewayReconciliationEffect, GatewayReconciliationEffectTiming,
        GatewayReconciliationIntent, GatewayReconciliationIntentDelivery,
        GatewayReconciliationOutcome, GatewayReconciliationRequest, ReconciliationHistoryFact,
    },
    domain::value_objects::{
        BundledSurface, ReconciliationCause, ReconciliationCleanupDecision,
        ReconciliationIncarnation, ReconciliationInitiator, ReconciliationPhysicalRecord,
        ReconciliationRuntimeIdentity, ReconciliationTarget, ReconciliationValidationFacts,
    },
};
use nessa_local_storage::PrivateTempFile;
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(in crate::gateway::infrastructure) struct FileReconciliationAudit {
    directory: Option<PathBuf>,
}

impl FileReconciliationAudit {
    pub(in crate::gateway::infrastructure) fn new(config_root: Option<PathBuf>) -> Self {
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
        let bytes = serde_json::to_vec(&value)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        write_new_record(
            &directory,
            &path,
            &bytes,
            nessa_local_storage::sync_directory,
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                GatewayError::Registration(
                    "Gateway reconciliation audit record already exists".into(),
                )
            } else {
                GatewayError::Registration(format!(
                    "Could not record gateway reconciliation: {error}"
                ))
            }
        })
    }
}

fn write_new_record(
    directory: &Path,
    destination: &Path,
    bytes: &[u8],
    sync_directory: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let mut temporary = PrivateTempFile::new_in(directory)?;
    temporary.as_file_mut().write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.publish(destination)?;
    sync_directory(directory)
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
            GatewayReconciliationEffect::Confirmed { history, .. } => {
                json!({
                    "kind":"confirmed",
                    "trustedHistory": history.facts().iter().map(|fact| native_fact(*fact)).collect::<Vec<_>>()
                })
            }
            GatewayReconciliationEffect::Failed { history, error } => {
                json!({
                    "kind":"failed",
                    "trustedHistory": history.facts().iter().map(|fact| native_fact(*fact)).collect::<Vec<_>>(),
                    "error":error.to_string()
                })
            }
            GatewayReconciliationEffect::RejectedReport {
                trusted_history,
                error,
                ..
            } => {
                json!({
                    "kind":"rejected_report",
                    "trustedHistory":trusted_history.facts().iter().map(|fact| native_fact(*fact)).collect::<Vec<_>>(),
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
                "intentDelivery":intent_delivery(outcome.intent_delivery()),
                "effectTiming":effect_timing(outcome.effect_timing()),
                "reportedHistory":outcome.reported_history().iter().map(|fact| native_fact(*fact)).collect::<Vec<_>>(),
                "reportedPhysical":reported_physical(outcome.physical()),
                "validation":validation(outcome.validation()),
                "cleanup":cleanup(outcome.cleanup()),
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

fn native_fact(fact: ReconciliationHistoryFact) -> &'static str {
    match fact {
        ReconciliationHistoryFact::RetirementAcknowledged => "retirement_acknowledged",
        ReconciliationHistoryFact::OldServiceUnloaded => "old_service_unloaded",
        ReconciliationHistoryFact::ServiceDefinitionPublished => "service_definition_published",
        ReconciliationHistoryFact::ServiceDefinitionDurable => "service_definition_durable",
        ReconciliationHistoryFact::BootstrapCommandRequested => "bootstrap_command_requested",
        ReconciliationHistoryFact::BootstrapCommandCompleted => "bootstrap_command_completed",
        ReconciliationHistoryFact::BootstrapCommandSucceeded => "bootstrap_command_succeeded",
    }
}

fn intent_delivery(delivery: &GatewayReconciliationIntentDelivery) -> Value {
    match delivery {
        GatewayReconciliationIntentDelivery::Reserved => json!({"kind":"reserved"}),
        GatewayReconciliationIntentDelivery::Acknowledged => json!({"kind":"acknowledged"}),
        GatewayReconciliationIntentDelivery::Failed(error) => {
            json!({"kind":"failed", "error":error.to_string()})
        }
    }
}

fn effect_timing(timing: GatewayReconciliationEffectTiming) -> &'static str {
    match timing {
        GatewayReconciliationEffectTiming::NoEffectsObserved => "no_effects_observed",
        GatewayReconciliationEffectTiming::BeforeIntentReservation => "before_intent_reservation",
        GatewayReconciliationEffectTiming::BeforeIntentAcknowledgement => {
            "before_intent_acknowledgement"
        }
        GatewayReconciliationEffectTiming::AfterIntentAcknowledgement => {
            "after_intent_acknowledgement"
        }
    }
}

fn reported_physical(physical: &ReconciliationPhysicalRecord) -> Value {
    match physical {
        ReconciliationPhysicalRecord::Confirmed(after) => {
            json!({"kind":"success", "claimedAfter":identity(after)})
        }
        ReconciliationPhysicalRecord::Failed => json!({"kind":"failed"}),
    }
}

fn validation(validation: &ReconciliationValidationFacts) -> Value {
    json!({
        "rejectedHistoryFact": validation.rejected_history_fact().map(native_fact),
        "historyComplete": validation.history_complete(),
        "targetMatches": validation.target_matches(),
        "runtimeIdentity": runtime_identity(validation.runtime_identity()),
        "physicalReportAgrees": validation.physical_report_agrees(),
        "effectTimingMatchesHistory": validation.effect_timing_matches_history(),
        "effectsFollowedIntent": validation.effects_followed_intent(),
        "candidateEligible": validation.candidate_eligible(),
    })
}

fn runtime_identity(identity: ReconciliationRuntimeIdentity) -> &'static str {
    match identity {
        ReconciliationRuntimeIdentity::NotReported => "not_reported",
        ReconciliationRuntimeIdentity::HealthyReuse => "healthy_reuse",
        ReconciliationRuntimeIdentity::FreshIncarnation => "fresh_incarnation",
        ReconciliationRuntimeIdentity::Replacement => "replacement",
        ReconciliationRuntimeIdentity::ReusedRuntimeInstance => "reused_runtime_instance",
        ReconciliationRuntimeIdentity::ChangedProcessForRuntimeInstance => {
            "changed_process_for_runtime_instance"
        }
    }
}

fn cleanup(cleanup: ReconciliationCleanupDecision) -> &'static str {
    match cleanup {
        ReconciliationCleanupDecision::RetainPrior => "retain_prior",
        ReconciliationCleanupDecision::ClearPrior => "clear_prior",
        ReconciliationCleanupDecision::AdoptClaimed => "adopt_claimed",
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
    use super::super::{
        bootstrap_result, bootstrap_succeeded, publish_definition, retire_then_unload,
        run_bootstrap,
    };
    use super::*;
    use crate::gateway::{
        application::{
            GatewayReconciliationAttempt, GatewayReconciliationIntentDelivery,
            GatewayReconciliationProgress, GatewayReconciliationRequest,
        },
        domain::value_objects::{
            ReconciliationCorrelation, ReconciliationEvidence, ReconciliationTarget,
        },
    };
    use std::{
        fs, io,
        os::unix::process::ExitStatusExt,
        process::{ExitStatus, Output},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Barrier, Mutex,
        },
        thread,
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

    fn temporary_root(name: &str) -> PathBuf {
        let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "nessa-reconciliation-audit-{name}-{}-{serial}",
            std::process::id()
        ))
    }

    #[derive(Default)]
    struct NativeProgress(Mutex<Vec<ReconciliationHistoryFact>>);

    impl NativeProgress {
        fn facts(&self) -> Vec<ReconciliationHistoryFact> {
            self.0.lock().expect("history").clone()
        }
    }

    impl GatewayReconciliationProgress for NativeProgress {
        fn readiness_invalidated(&self) {}

        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }

        fn history_observed(&self, fact: ReconciliationHistoryFact) {
            self.0.lock().expect("history").push(fact);
        }
    }

    fn target() -> ReconciliationTarget {
        ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("target")
    }

    fn incarnation(
        target: ReconciliationTarget,
        instance: &str,
        process_id: u32,
    ) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(target, instance.into(), process_id, 7420)
            .expect("incarnation")
    }

    fn replacement_intent(value: u64) -> GatewayReconciliationIntent {
        let target = target();
        let before = incarnation(target.clone(), "550e8400-e29b-41d4-a716-446655440000", 41);
        let attempt = GatewayReconciliationAttempt::new(
            correlation(value),
            request(value + 100, ReconciliationCause::Startup),
        )
        .expect("attempt");
        GatewayReconciliationIntent::new(attempt, target, Some(before)).expect("intent")
    }

    fn record_failed_native_attempt(
        audit: &FileReconciliationAudit,
        intent: GatewayReconciliationIntent,
        progress: &NativeProgress,
        error: &str,
    ) -> Value {
        let correlation = intent.attempt().correlation().as_str().to_owned();
        let facts = progress.facts();
        let effect_timing = if facts.is_empty() {
            GatewayReconciliationEffectTiming::NoEffectsObserved
        } else {
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement
        };
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            effect_timing,
            facts,
            Err(GatewayError::Registration(error.into())),
        );
        audit.outcome(&outcome).expect("outcome write");
        let directory = audit.directory().expect("audit directory");
        let recorded: Value = serde_json::from_slice(
            &fs::read(directory.join(format!("{correlation}-outcome.json"))).expect("record"),
        )
        .expect("json");
        assert_eq!(recorded["intentDelivery"]["kind"], "acknowledged");
        assert_eq!(recorded["effect"]["kind"], "failed");
        recorded
    }

    fn complete_installation_through_publish(progress: &NativeProgress) {
        retire_then_unload(progress, || Ok(()), || Ok(())).expect("retire and unload");
        publish_definition(progress, || Ok(()), || Ok(())).expect("publish and sync");
    }

    #[test]
    fn native_helper_failures_are_assessed_and_serialized_with_distinct_cleanup() {
        let root = temporary_root("native-failures");
        let _ = fs::remove_dir_all(&root);
        let audit = FileReconciliationAudit::new(Some(root.clone()));

        let retirement = NativeProgress::default();
        assert_eq!(
            retire_then_unload(&retirement, || Err("retirement failed".into()), || Ok(())),
            Err("retirement failed".into())
        );
        let retirement = record_failed_native_attempt(
            &audit,
            replacement_intent(30),
            &retirement,
            "retirement failed",
        );
        assert_eq!(retirement["reportedHistory"], json!([]));
        assert_eq!(retirement["reportedPhysical"]["kind"], "failed");
        assert_eq!(retirement["cleanup"], "retain_prior");

        let bootout = NativeProgress::default();
        assert_eq!(
            retire_then_unload(&bootout, || Ok(()), || Err("bootout failed".into())),
            Err("bootout failed".into())
        );
        let bootout = record_failed_native_attempt(
            &audit,
            replacement_intent(31),
            &bootout,
            "bootout failed",
        );
        assert_eq!(
            bootout["reportedHistory"],
            json!(["retirement_acknowledged"])
        );
        assert_eq!(bootout["cleanup"], "retain_prior");

        let rename = NativeProgress::default();
        retire_then_unload(&rename, || Ok(()), || Ok(())).expect("retire and unload");
        assert_eq!(
            publish_definition(&rename, || Err("rename failed".into()), || Ok(())),
            Err("rename failed".into())
        );
        let rename =
            record_failed_native_attempt(&audit, replacement_intent(32), &rename, "rename failed");
        assert_eq!(
            rename["reportedHistory"],
            json!(["retirement_acknowledged", "old_service_unloaded"])
        );
        assert_eq!(rename["cleanup"], "clear_prior");

        let fsync = NativeProgress::default();
        retire_then_unload(&fsync, || Ok(()), || Ok(())).expect("retire and unload");
        assert_eq!(
            publish_definition(&fsync, || Ok(()), || Err("fsync failed".into())),
            Err("fsync failed".into())
        );
        let fsync =
            record_failed_native_attempt(&audit, replacement_intent(33), &fsync, "fsync failed");
        assert_eq!(
            fsync["reportedHistory"],
            json!([
                "retirement_acknowledged",
                "old_service_unloaded",
                "service_definition_published"
            ])
        );
        assert_eq!(fsync["cleanup"], "clear_prior");

        let spawn = NativeProgress::default();
        complete_installation_through_publish(&spawn);
        assert_eq!(
            run_bootstrap(&spawn, || Err::<(), _>("spawn failed")),
            Err("spawn failed")
        );
        let spawn =
            record_failed_native_attempt(&audit, replacement_intent(34), &spawn, "spawn failed");
        assert_eq!(
            spawn["reportedHistory"].as_array().expect("history").last(),
            Some(&json!("bootstrap_command_requested"))
        );
        assert_eq!(spawn["cleanup"], "clear_prior");

        let nonzero = NativeProgress::default();
        complete_installation_through_publish(&nonzero);
        let bootstrap = run_bootstrap(&nonzero, || {
            Ok::<_, io::Error>(Output {
                status: ExitStatus::from_raw(1 << 8),
                stdout: Vec::new(),
                stderr: b"refused".to_vec(),
            })
        })
        .expect("command completed");
        let refusal = bootstrap_result(bootstrap).expect_err("nonzero completion");
        let nonzero = record_failed_native_attempt(
            &audit,
            replacement_intent(35),
            &nonzero,
            &refusal.to_string(),
        );
        assert_eq!(
            nonzero["reportedHistory"]
                .as_array()
                .expect("history")
                .last(),
            Some(&json!("bootstrap_command_completed"))
        );
        assert_eq!(nonzero["cleanup"], "clear_prior");

        let health = NativeProgress::default();
        complete_installation_through_publish(&health);
        run_bootstrap(&health, || Ok::<_, io::Error>(())).expect("bootstrap");
        bootstrap_succeeded(&health);
        let health = record_failed_native_attempt(
            &audit,
            replacement_intent(36),
            &health,
            "replacement did not become healthy",
        );
        assert_eq!(
            health["reportedHistory"]
                .as_array()
                .expect("history")
                .last(),
            Some(&json!("bootstrap_command_succeeded"))
        );
        assert_eq!(health["reportedPhysical"]["kind"], "failed");
        assert_eq!(health["effect"]["kind"], "failed");
        assert_eq!(health["cleanup"], "clear_prior");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejected_physical_claims_keep_identity_and_never_gain_cleanup_eligibility() {
        let root = temporary_root("rejected-claims");
        let _ = fs::remove_dir_all(&root);
        let audit = FileReconciliationAudit::new(Some(root.clone()));

        let progress = NativeProgress::default();
        complete_installation_through_publish(&progress);
        run_bootstrap(&progress, || Ok::<_, io::Error>(())).expect("bootstrap");
        bootstrap_succeeded(&progress);
        let intent = replacement_intent(40);
        let claimed = incarnation(
            ReconciliationTarget::new(
                "gui/501/so.nessa.gateway.prod".into(),
                "c".repeat(64),
                "d".repeat(64),
            )
            .expect("conflicting target"),
            "550e8400-e29b-41d4-a716-446655440099",
            99,
        );
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement,
            progress.facts(),
            Ok(claimed),
        );
        audit.outcome(&outcome).expect("outcome write");
        let path = audit
            .directory()
            .expect("directory")
            .join("00000000-0000-4000-8000-000000000028-outcome.json");
        let recorded: Value =
            serde_json::from_slice(&fs::read(path).expect("record")).expect("json");
        assert_eq!(recorded["effect"]["kind"], "rejected_report");
        assert_eq!(recorded["reportedPhysical"]["kind"], "success");
        assert_eq!(
            recorded["reportedPhysical"]["claimedAfter"]["processId"],
            99
        );
        assert_eq!(
            recorded["reportedPhysical"]["claimedAfter"]["runtimeInstance"],
            "550e8400-e29b-41d4-a716-446655440099"
        );
        assert_eq!(recorded["validation"]["targetMatches"], false);
        assert_eq!(recorded["validation"]["candidateEligible"], false);
        assert_eq!(recorded["cleanup"], "clear_prior");

        let intent = replacement_intent(41);
        let claimed = incarnation(target(), "550e8400-e29b-41d4-a716-446655440000", 99);
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement,
            progress.facts(),
            Ok(claimed),
        );
        audit.outcome(&outcome).expect("outcome write");
        let path = audit
            .directory()
            .expect("directory")
            .join("00000000-0000-4000-8000-000000000029-outcome.json");
        let recorded: Value =
            serde_json::from_slice(&fs::read(path).expect("record")).expect("json");
        assert_eq!(
            recorded["validation"]["runtimeIdentity"],
            "changed_process_for_runtime_instance"
        );
        assert_eq!(recorded["validation"]["candidateEligible"], false);
        assert_eq!(recorded["cleanup"], "clear_prior");
        assert_eq!(
            recorded["reportedPhysical"]["claimedAfter"]["runtimeInstance"],
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(
            recorded["reportedPhysical"]["claimedAfter"]["processId"],
            99
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_intent_sink_is_serialized_apart_from_the_unrun_native_effect() {
        let intent = replacement_intent(50);
        let delivery_error = FileReconciliationAudit::new(None)
            .intent(&intent)
            .expect_err("missing audit storage");
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Failed(delivery_error),
            GatewayReconciliationEffectTiming::NoEffectsObserved,
            Vec::new(),
            Err(GatewayError::Registration(
                "native helper was not run".into(),
            )),
        );
        let root = temporary_root("sink-failure");
        let _ = fs::remove_dir_all(&root);
        let audit = FileReconciliationAudit::new(Some(root.clone()));
        audit.outcome(&outcome).expect("outcome write");
        let path = audit
            .directory()
            .expect("directory")
            .join("00000000-0000-4000-8000-000000000032-outcome.json");
        let recorded: Value =
            serde_json::from_slice(&fs::read(path).expect("record")).expect("json");
        assert_eq!(recorded["intentDelivery"]["kind"], "failed");
        assert_eq!(
            recorded["intentDelivery"]["error"],
            "Application config storage is required for gateway audit"
        );
        assert_eq!(recorded["effect"]["error"], "native helper was not run");
        assert_eq!(recorded["reportedHistory"], json!([]));
        assert_eq!(recorded["cleanup"], "retain_prior");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn late_intent_acknowledgement_is_distinct_from_effect_ordering() {
        let root = temporary_root("late-intent-acknowledgement");
        let _ = fs::remove_dir_all(&root);
        let audit = FileReconciliationAudit::new(Some(root.clone()));
        let progress = NativeProgress::default();
        complete_installation_through_publish(&progress);
        run_bootstrap(&progress, || Ok::<_, io::Error>(())).expect("bootstrap");
        bootstrap_succeeded(&progress);
        let intent = replacement_intent(60);
        let correlation = intent.attempt().correlation().as_str().to_owned();
        let after = incarnation(target(), "550e8400-e29b-41d4-a716-446655440060", 60);
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::BeforeIntentAcknowledgement,
            progress.facts(),
            Ok(after),
        );
        audit.outcome(&outcome).expect("outcome write");
        let recorded: Value = serde_json::from_slice(
            &fs::read(
                audit
                    .directory()
                    .expect("directory")
                    .join(format!("{correlation}-outcome.json")),
            )
            .expect("record"),
        )
        .expect("json");
        assert_eq!(recorded["intentDelivery"]["kind"], "acknowledged");
        assert_eq!(recorded["effectTiming"], "before_intent_acknowledgement");
        assert_eq!(recorded["validation"]["effectTimingMatchesHistory"], true);
        assert_eq!(recorded["validation"]["effectsFollowedIntent"], false);
        assert_eq!(recorded["validation"]["candidateEligible"], true);
        assert_eq!(recorded["effect"]["kind"], "rejected_report");
        assert_eq!(recorded["cleanup"], "adopt_claimed");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn concurrent_writers_cannot_replace_an_existing_audit_record() {
        let root = temporary_root("exclusive");
        let _ = fs::remove_dir_all(&root);
        let audit = Arc::new(FileReconciliationAudit::new(Some(root.clone())));
        let intents = [21_u64, 22]
            .map(|origin| {
                let attempt = GatewayReconciliationAttempt::new(
                    correlation(20),
                    request(origin, ReconciliationCause::Startup),
                )
                .expect("attempt");
                let target = ReconciliationTarget::new(
                    "gui/501/so.nessa.gateway.prod".into(),
                    if origin == 21 {
                        "a".repeat(64)
                    } else {
                        "c".repeat(64)
                    },
                    "b".repeat(64),
                )
                .expect("target");
                Arc::new(GatewayReconciliationIntent::new(attempt, target, None).expect("intent"))
            })
            .to_vec();
        let barrier = Arc::new(Barrier::new(3));
        let writers = intents
            .iter()
            .map(|intent| {
                let audit = Arc::clone(&audit);
                let intent = Arc::clone(intent);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    audit.intent(&intent)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = writers
            .into_iter()
            .map(|writer| writer.join().expect("writer"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);

        let path = root
            .join("gateway-reconciliation-audit/00000000-0000-4000-8000-000000000014-intent.json");
        let original = fs::read(&path).expect("record remains");
        let recorded: Value = serde_json::from_slice(&original).expect("json");
        let origin = recorded["origin"]["correlation"].as_str().expect("origin");
        let fingerprint = recorded["target"]["runtimeFingerprint"]
            .as_str()
            .expect("fingerprint");
        assert!(
            (origin.ends_with("000000000015") && fingerprint == "a".repeat(64))
                || (origin.ends_with("000000000016") && fingerprint == "c".repeat(64))
        );
        let loser = if origin.ends_with("000000000015") {
            &intents[1]
        } else {
            &intents[0]
        };
        audit.intent(loser).expect_err("record name remains taken");
        assert_eq!(fs::read(&path).expect("unchanged record"), original);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn directory_sync_failure_reports_failure_without_replacing_the_published_record() {
        let root = temporary_root("sync-failure");
        let _ = fs::remove_dir_all(&root);
        nessa_local_storage::create_directory(&root).expect("directory");
        let path = root.join("outcome.json");
        let error = write_new_record(&root, &path, br#"{"kind":"failed"}"#, |_| {
            Err(io::Error::other("sync failed"))
        })
        .expect_err("sync must fail");
        assert_eq!(error.to_string(), "sync failed");
        assert_eq!(
            fs::read(&path).expect("published record"),
            br#"{"kind":"failed"}"#
        );

        let taken = write_new_record(&root, &path, br#"{"kind":"replacement"}"#, |_| Ok(()))
            .expect_err("published record must remain exclusive");
        assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&path).expect("original record"),
            br#"{"kind":"failed"}"#
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn records_intent_outcome_and_join_without_overwriting() {
        let root = temporary_root("records");
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

        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::NoEffectsObserved,
            Vec::new(),
            Err(GatewayError::Registration("refused".into())),
        );
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
        assert_eq!(recorded["effect"]["kind"], "failed");

        let attempt = GatewayReconciliationAttempt::new(
            correlation(5),
            request(4, ReconciliationCause::Startup),
        )
        .expect("attempt");
        let target = ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("target");
        let after = ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            42,
            7420,
        )
        .expect("identity");
        let intent =
            GatewayReconciliationIntent::new(attempt, target, Some(after.clone())).expect("intent");
        let rejected = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement,
            vec![ReconciliationHistoryFact::BootstrapCommandSucceeded],
            Ok(after),
        );
        audit.outcome(&rejected).expect("rejected write");
        let rejected: Value = serde_json::from_slice(
            &fs::read(directory.join("00000000-0000-4000-8000-000000000005-outcome.json"))
                .expect("record"),
        )
        .expect("json");
        assert_eq!(rejected["effect"]["kind"], "rejected_report");
        assert_eq!(
            rejected["validation"]["rejectedHistoryFact"],
            "bootstrap_command_succeeded"
        );
        assert_eq!(rejected["validation"]["candidateEligible"], false);
        assert_eq!(rejected["cleanup"], "retain_prior");
        assert_eq!(rejected["reportedPhysical"]["kind"], "success");
        assert_eq!(
            rejected["reportedPhysical"]["claimedAfter"]["processId"],
            42
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
