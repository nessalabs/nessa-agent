//! launchd registration and loopback readiness. Service lifetime belongs to launchd.
use crate::gateway::application::{
    GatewayError, GatewayHost, GatewayLifecycleRecovery, GatewayPhysicalResult,
    GatewayReconciliationAttempt, GatewayReconciliationIntent, GatewayReconciliationJournalSession,
    GatewayReconciliationProgress, GatewayStopSession, ReconciledGateway,
    ReconciliationHistoryFact, StartupStep,
};
use crate::gateway::domain::value_objects::{
    AuditDeliveryReceipt, LifecycleCommandResult, LifecycleEffect, LifecycleEffectPredicate,
    LifecycleFailedPhase, LifecycleObservation, LifecycleObservationSource,
    LifecyclePhysicalOutcome, LifecyclePlanStep, ReconciliationCause,
    ReconciliationCleanupDecision, ReconciliationIncarnation, ReconciliationTarget, SearchPath,
    ServiceConfiguration,
};
use nessa_local_storage::{OpenMode, PrivateDirectory};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fmt::{self, Display, Formatter},
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Arc,
    time::Duration,
};

mod control;
mod generation;
mod pruning;
mod staging;
mod startup;
use control::{
    bounded_output, classify, forward_recovery, health, launchctl, legacy_listener_pid,
    lock_namespace, read_pending_retirement, read_retirement_evidence, retire, service_status,
    wait_fingerprint, Health, InstallFailure, ManagedRuntime, Registration, ServiceState,
    ServiceStatus, LAUNCHCTL_DEADLINE,
};
use generation::service_generation;
use pruning::{prune_runtime, removable_runtime_names, retained_runtimes, RetainedRuntimes};
use staging::{launch_settings, stage_runtime_cached, ValidatedRuntimes};

/// launchd and the loopback health endpoint: what the bootstrap step and
/// recovery read, and the two commands the bootstrap step runs.
pub(super) trait Launchctl: Send + Sync {
    fn status(&self, service: &str) -> Result<ServiceStatus, String>;
    fn health(&self, port: u16) -> Option<Health>;
    /// `launchctl bootstrap <domain> <plist>`, with its raw output.
    fn bootstrap(&self, domain: &str, plist: &Path) -> std::io::Result<Output>;
    /// `launchctl bootout <service>`.
    fn bootout(&self, service: &str) -> Result<(), String>;
    /// `launchctl kill <signal> <service>`. `keep_waiting` is asked while
    /// launchctl has not returned; once it answers false the command is
    /// abandoned and its result is indeterminate.
    fn signal(
        &self,
        service: &str,
        signal: &str,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> LifecycleCommandResult;
}

pub(super) struct NativeLaunchctl;

impl Launchctl for NativeLaunchctl {
    fn status(&self, service: &str) -> Result<ServiceStatus, String> {
        service_status(service)
    }
    fn health(&self, port: u16) -> Option<Health> {
        health(port)
    }
    fn bootstrap(&self, domain: &str, plist: &Path) -> std::io::Result<Output> {
        bounded_output(
            Command::new("/bin/launchctl")
                .args(["bootstrap", domain])
                .arg(plist)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            LAUNCHCTL_DEADLINE,
        )
        .map_err(std::io::Error::other)
    }
    fn bootout(&self, service: &str) -> Result<(), String> {
        launchctl(&["bootout", service])
    }
    fn signal(
        &self,
        service: &str,
        signal: &str,
        keep_waiting: &mut dyn FnMut() -> bool,
    ) -> LifecycleCommandResult {
        let mut child = match Command::new("/bin/launchctl")
            .args(["kill", signal, service])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return LifecycleCommandResult::Failed(error.to_string()),
        };
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return LifecycleCommandResult::Accepted,
                Ok(Some(_)) => {
                    let output = child.wait_with_output();
                    return LifecycleCommandResult::Rejected(
                        output
                            .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_owned())
                            .unwrap_or_else(|error| error.to_string()),
                    );
                }
                Ok(None) if keep_waiting() => {}
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return LifecycleCommandResult::Indeterminate(
                        "launchctl did not return before the quit deadline".into(),
                    );
                }
                Err(error) => return LifecycleCommandResult::Indeterminate(error.to_string()),
            }
        }
    }
}

/// Where one launchd registration keeps what its steps observe, and what
/// each step's observation means. The live steps and recovery record the same
/// fact for the same step.
struct LaunchdArtifacts {
    runtimes: PathBuf,
    plist: PathBuf,
}

impl LaunchdArtifacts {
    fn for_label(home: &Path, label: &str) -> Self {
        Self {
            runtimes: home
                .join("Library/Application Support/Nessa/gateway-runtimes")
                .join(label),
            plist: home
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
        }
    }

    /// Whether `effect`'s artifact is present: the runtime directory for
    /// staging and pruning, the staging directory for staging cleanup, the
    /// plist for publication and retirement, and whether launchd has the
    /// label loaded for bootstrap, unload, adoption and agent stop. Every
    /// launchd observation, live or recovered, records this.
    /// A file that cannot be examined is an error, never "absent".
    fn present(&self, effect: &LifecycleEffect, loaded: bool) -> Result<bool, String> {
        match effect {
            LifecycleEffect::StageRuntime { fingerprint }
            | LifecycleEffect::PruneRuntime { fingerprint } => {
                artifact_presence(&self.runtimes.join(fingerprint))
            }
            LifecycleEffect::RemoveStagingRuntime { generation } => {
                artifact_presence(&self.runtimes.join(format!(".staging-{generation}")))
            }
            LifecycleEffect::PublishServiceDefinition { .. }
            | LifecycleEffect::RequestRetirement { .. } => artifact_presence(&self.plist),
            LifecycleEffect::BootstrapService { .. }
            | LifecycleEffect::UnloadService { .. }
            | LifecycleEffect::AdoptReadyIncarnation { .. }
            | LifecycleEffect::StopAgents { .. } => Ok(loaded),
            // Not a launchd effect: recovery refuses it before observing.
            _ => Err("Not a launchd effect".into()),
        }
    }
}

pub(super) struct Launchd {
    launchctl: Arc<dyn Launchctl>,
    validated_runtimes: ValidatedRuntimes,
    disabled_services: Arc<dyn DisabledServiceStatus>,
    configuration: ServiceConfiguration,
    home: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BootstrapRecoveryDecision {
    Adopt(ReconciliationIncarnation),
    Refuse,
}

/// The planned target, when launchd runs it: `observed_incarnation` already
/// requires the label loaded with the healthy process's PID, so the target is
/// all that is left to compare. The one definition of "exact target running".
fn exact_target<'a>(
    observed: Option<&'a ReconciliationIncarnation>,
    target: &ReconciliationTarget,
) -> Option<&'a ReconciliationIncarnation> {
    observed.filter(|incarnation| incarnation.target() == target)
}

fn bootstrap_recovery_decision(
    observed: Option<&ReconciliationIncarnation>,
    target: &ReconciliationTarget,
) -> BootstrapRecoveryDecision {
    match exact_target(observed, target) {
        Some(incarnation) => BootstrapRecoveryDecision::Adopt(incarnation.clone()),
        None => BootstrapRecoveryDecision::Refuse,
    }
}

fn installed_target_port(definition: Option<&Value>, target: &ReconciliationTarget) -> Option<u16> {
    let environment = definition?.get("EnvironmentVariables")?;
    if environment.get("NESSA_RUNTIME_FINGERPRINT")?.as_str()? != target.runtime_fingerprint()
        || environment.get("NESSA_SERVICE_GENERATION")?.as_str()? != target.service_generation()
    {
        return None;
    }
    environment
        .get("NESSA_PORT")?
        .as_str()?
        .parse()
        .ok()
        .filter(|port| *port != 0)
}

fn recovery_probe_port(
    target: &ReconciliationTarget,
    latest: Option<&LifecycleObservation>,
    definition: Option<&Value>,
    before: Option<&ReconciliationIncarnation>,
    fallback: u16,
) -> u16 {
    latest
        .and_then(LifecycleObservation::incarnation)
        .filter(|incarnation| incarnation.target() == target)
        .map(ReconciliationIncarnation::port)
        .or_else(|| installed_target_port(definition, target))
        .or_else(|| {
            before
                .filter(|incarnation| incarnation.target() == target)
                .map(ReconciliationIncarnation::port)
        })
        .unwrap_or(fallback)
}

impl Launchd {
    pub(super) fn new(
        disabled_services: Arc<dyn DisabledServiceStatus>,
        launchctl: Arc<dyn Launchctl>,
        configuration: ServiceConfiguration,
        home: PathBuf,
    ) -> Self {
        Self {
            launchctl,
            validated_runtimes: ValidatedRuntimes::default(),
            disabled_services,
            configuration,
            home,
        }
    }
}
impl GatewayHost for Launchd {
    fn startup_cause(&self) -> ReconciliationCause {
        if provider_configuration_changed(&self.configuration, &self.home) {
            ReconciliationCause::ClaudeConfigurationChanged
        } else {
            ReconciliationCause::Startup
        }
    }

    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        register(self, runtime, stage, agent_path, attempt, progress).map_err(|error| match error {
            RegisterFailure::Physical(message) => GatewayError::Registration(message),
            RegisterFailure::Audit(error) => error,
        })
    }

    fn recover(
        &self,
        recovery: &GatewayLifecycleRecovery,
        journal: &dyn GatewayReconciliationJournalSession,
    ) -> Result<(), GatewayError> {
        let target = recovery.target();
        let prefix = format!("gui/{}/", unsafe { libc::getuid() });
        let label = target
            .service()
            .strip_prefix(&prefix)
            .filter(|label| label.starts_with("so.nessa.gateway.") && !label.contains('/'))
            .ok_or_else(|| {
                GatewayError::Registration(
                    "The unresolved gateway namespace is not owned by this desktop host".into(),
                )
            })?;
        let definition = read_definition(
            &self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
        )
        .ok();
        let port = recovery_probe_port(
            target,
            recovery.latest_observation(),
            definition.as_ref(),
            recovery.before(),
            self.configuration.port(),
        );
        let status = self
            .launchctl
            .status(target.service())
            .map_err(GatewayError::Registration)?;
        let observed =
            observed_incarnation(target.service(), port, &status, self.launchctl.health(port));
        // Whether the target is present for an attempt closed before any
        // plan: launchd runs it, or the installed plist carries its
        // generation, and it was not the prior.
        let target_artifact_present = (observed
            .as_ref()
            .is_some_and(|incarnation| incarnation.target() == target)
            || installed_generation(definition.as_ref()) == Some(target.service_generation()))
            && !recovery
                .before()
                .is_some_and(|incarnation| incarnation.target() == target);
        let artifacts = LaunchdArtifacts::for_label(&self.home, label);
        let settle = |decided: Option<LifecycleCommandResult>, observed| {
            settle_without_replay(recovery, journal, decided, observed, |effect| {
                artifacts.present(effect, status.loaded)
            })
        };
        let close = |settled: Settled, message: &str| close_failed(journal, settled, message);
        let Some(step) = recovery.pending_step() else {
            if !recovery.has_effect_plan() {
                // No plan authorized no effect, so whatever is there now was
                // not caused by this attempt.
                let observation = LifecycleObservation::new(
                    recovery
                        .latest_observation()
                        .map_or(1, |prior| prior.version().saturating_add(1)),
                    observed,
                    target_artifact_present,
                );
                retry_journal_delivery(|| {
                    journal.observation(&LifecycleObservationSource::Intent, &observation)
                })?;
                return close(
                    Settled {
                        observation,
                        unreturned: false,
                    },
                    "Recovered admitted intent before any effect-capable plan",
                );
            }
            // Every plan settled and was observed. A step's observation
            // records only that step's artifact, so it is not compared with a
            // fresh one.
            let observation = recovery.latest_observation().cloned().ok_or_else(|| {
                GatewayError::Registration(
                    "The unresolved gateway plan has no completion observation and requires exact resume"
                        .into(),
                )
            })?;
            return close(
                Settled {
                    observation,
                    unreturned: false,
                },
                "Recovered settled launchd plans from their last saved observation; nothing was replayed",
            );
        };
        match step.step().effect() {
            LifecycleEffect::BootstrapService { target: planned } => {
                // launchd's recorded refusal is never adopted, whatever runs now.
                let refused = step
                    .completion()
                    .is_some_and(|result| !matches!(result, LifecycleCommandResult::Accepted));
                let decision = if refused {
                    BootstrapRecoveryDecision::Refuse
                } else {
                    bootstrap_recovery_decision(observed.as_ref(), planned)
                };
                let BootstrapRecoveryDecision::Adopt(exact) = decision else {
                    return close(
                        settle(None, observed)?,
                        "Recovered bootstrap without its exact planned target; nothing was booted out",
                    );
                };
                let settled = settle(None, observed)?;
                retry_journal_delivery(|| {
                    journal.physical_outcome(
                        &LifecyclePhysicalOutcome::Confirmed(exact.clone()),
                        Some(&settled.observation),
                        ReconciliationCleanupDecision::AdoptClaimed,
                    )
                })?;
                Ok(())
            }
            LifecycleEffect::AdoptReadyIncarnation { target: planned } => {
                let exact = matches!(
                    bootstrap_recovery_decision(observed.as_ref(), planned),
                    BootstrapRecoveryDecision::Adopt(_)
                );
                let settled = settle_without_replay(
                    recovery,
                    journal,
                    Some(if exact {
                        LifecycleCommandResult::Accepted
                    } else {
                        LifecycleCommandResult::Failed(
                            "The planned gateway incarnation was not ready at recovery".into(),
                        )
                    }),
                    observed.clone(),
                    |effect| artifacts.present(effect, status.loaded),
                )?;
                let physical = match observed.filter(|_| exact) {
                    Some(incarnation) => LifecyclePhysicalOutcome::Confirmed(incarnation),
                    None => LifecyclePhysicalOutcome::Failed {
                        phase: if settled.unreturned {
                            LifecycleFailedPhase::NativeDispatch
                        } else {
                            LifecycleFailedPhase::Observation
                        },
                        message: "Recovered readiness plan did not find its exact incarnation"
                            .into(),
                    },
                };
                retry_journal_delivery(|| {
                    journal.physical_outcome(
                        &physical,
                        Some(&settled.observation),
                        if exact {
                            ReconciliationCleanupDecision::AdoptClaimed
                        } else {
                            ReconciliationCleanupDecision::RetainPrior
                        },
                    )
                })?;
                Ok(())
            }
            LifecycleEffect::StopAgents { incarnation } => {
                let command = step.completion().cloned().unwrap_or_else(|| {
                    LifecycleCommandResult::Indeterminate(
                        "The stop dispatch result was not recorded before recovery".into(),
                    )
                });
                let settled = settle_without_replay(
                    recovery,
                    journal,
                    Some(command.clone()),
                    observed,
                    |effect| artifacts.present(effect, status.loaded),
                )?;
                retry_journal_delivery(|| {
                    journal.physical_outcome(
                        &LifecyclePhysicalOutcome::StopAgentsSettled {
                            intended: incarnation.clone(),
                            command: command.clone(),
                            observed: settled.observation.clone(),
                        },
                        Some(&settled.observation),
                        ReconciliationCleanupDecision::RetainPrior,
                    )
                })?;
                Ok(())
            }
            LifecycleEffect::UnloadService { .. } => close(
                settle(None, observed)?,
                "Recovered an interrupted unload without replaying it",
            ),
            LifecycleEffect::StageRuntime { .. }
            | LifecycleEffect::PublishServiceDefinition { .. }
            | LifecycleEffect::RequestRetirement { .. }
            | LifecycleEffect::PruneRuntime { .. }
            | LifecycleEffect::RemoveStagingRuntime { .. } => close(
                settle(None, observed)?,
                "Recovered an interrupted launchd step without replaying it",
            ),
            _ => Err(GatewayError::Registration(
                "The unresolved gateway effect is not a launchd effect this host can recover"
                    .into(),
            )),
        }
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        let token = session.begin_proof()?;
        let gateway = session.request().intended();
        let status = self
            .launchctl
            .status(gateway.service())
            .map_err(GatewayError::Stop)?;
        let running = self.launchctl.health(gateway.port());
        if !matches_reconciled_gateway(gateway, &status, running.as_ref()) {
            return Err(GatewayError::Stop(
                "Gateway runtime identity changed; no agent stop request was sent".into(),
            ));
        }
        let candidate = gateway.audit_identity()?;
        let observation_version = 1;
        session.prove(&token, candidate.clone(), observation_version)?;
        session.claim(token, plan, &candidate, observation_version)?;
        // Claim and spawn are adjacent: no filesystem, lock, health, or manager
        // query can invalidate an unconsumed proof between them.
        let command = dispatch_stop_command(self.launchctl.as_ref(), gateway.service(), session);
        session.command_result(command.clone())?;
        retry_journal_delivery(|| {
            journal.effect_completion("stop-agents-on-desktop-quit", "signal-agents", &command)
        })?;
        if matches!(command, LifecycleCommandResult::Indeterminate(_)) {
            return Err(GatewayError::Stop(
                "Gateway stop command was indeterminate at the quit deadline".into(),
            ));
        }
        if session.deadline_passed() {
            return Err(GatewayError::Stop(
                "Gateway stop deadline passed before fresh observation".into(),
            ));
        }
        let status = self
            .launchctl
            .status(gateway.service())
            .map_err(GatewayError::Stop)?;
        let running = self.launchctl.health(gateway.port());
        let observed = observed_incarnation(gateway.service(), gateway.port(), &status, running);
        let stop = LifecycleEffect::StopAgents {
            incarnation: candidate.clone(),
        };
        let label = gateway.service().rsplit('/').next().unwrap_or_default();
        let observation = LifecycleObservation::new(
            2,
            observed,
            LaunchdArtifacts::for_label(&self.home, label)
                .present(&stop, status.loaded)
                .map_err(GatewayError::Stop)?,
        );
        let source = LifecycleObservationSource::Effect {
            plan_id: "stop-agents-on-desktop-quit".into(),
            step_id: "signal-agents".into(),
        };
        retry_journal_delivery(|| journal.observation(&source, &observation))?;
        session.fresh_observation(observation.clone())?;
        match command {
            LifecycleCommandResult::Accepted => Ok(observation),
            LifecycleCommandResult::Rejected(message) | LifecycleCommandResult::Failed(message) => {
                Err(GatewayError::Stop(message))
            }
            LifecycleCommandResult::Indeterminate(_) => unreachable!(),
        }
    }
}

fn retry_journal_delivery<T>(
    mut deliver: impl FnMut() -> Result<T, GatewayError>,
) -> Result<T, GatewayError> {
    match deliver() {
        Ok(receipt) => Ok(receipt),
        Err(_) => deliver(),
    }
}

fn dispatch_stop_command(
    launchctl: &dyn Launchctl,
    service: &str,
    session: &GatewayStopSession,
) -> LifecycleCommandResult {
    launchctl.signal(service, "SIGUSR1", &mut || {
        if session.deadline_passed() {
            return false;
        }
        session.wait(Duration::from_millis(10));
        true
    })
}

fn observed_incarnation(
    service: &str,
    port: u16,
    status: &ServiceStatus,
    health: Option<Health>,
) -> Option<ReconciliationIncarnation> {
    let Health::Managed(runtime) = health? else {
        return None;
    };
    if !status.loaded || !status.process_identity_known || status.pid != Some(runtime.pid) {
        return None;
    }
    let target =
        ReconciliationTarget::new(service.to_owned(), runtime.fingerprint, runtime.generation)
            .ok()?;
    ReconciliationIncarnation::new(target, runtime.instance, runtime.pid, port).ok()
}

fn matches_reconciled_gateway(
    gateway: &ReconciledGateway,
    status: &ServiceStatus,
    health: Option<&Health>,
) -> bool {
    matches!(
        health,
        Some(Health::Managed(runtime))
            if status.loaded
                && status.process_identity_known
                && status.pid == Some(gateway.process_id())
                && runtime.pid == gateway.process_id()
                && runtime.fingerprint == gateway.runtime_fingerprint()
                && runtime.instance == gateway.runtime_instance()
                && runtime.generation == gateway.service_generation()
    )
}

#[cfg(test)]
fn retire_then_unload(
    progress: &dyn GatewayReconciliationProgress,
    retire: impl FnOnce() -> Result<(), String>,
    unload: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    retire()?;
    progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
    unload()?;
    progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
    Ok(())
}

fn publish_definition(
    progress: &dyn GatewayReconciliationProgress,
    rename: impl FnOnce() -> Result<(), String>,
    sync_directory: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    rename()?;
    progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionPublished);
    sync_directory()?;
    progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionDurable);
    Ok(())
}

fn run_bootstrap<T, E>(
    progress: &dyn GatewayReconciliationProgress,
    run: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    progress.history_observed(ReconciliationHistoryFact::BootstrapCommandRequested);
    let output = run()?;
    progress.history_observed(ReconciliationHistoryFact::BootstrapCommandCompleted);
    Ok(output)
}

fn bootstrap_succeeded(progress: &dyn GatewayReconciliationProgress) {
    progress.history_observed(ReconciliationHistoryFact::BootstrapCommandSucceeded);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapCleanupDecision {
    UnloadExactTarget,
    AlreadyAbsent,
    RefuseReplacement,
}

fn bootstrap_cleanup_decision(
    loaded: bool,
    observed: Option<&ReconciliationIncarnation>,
    target: &ReconciliationTarget,
) -> BootstrapCleanupDecision {
    if exact_target(observed, target).is_some() {
        BootstrapCleanupDecision::UnloadExactTarget
    } else if loaded {
        BootstrapCleanupDecision::RefuseReplacement
    } else {
        BootstrapCleanupDecision::AlreadyAbsent
    }
}

fn cleanup_bootstrap(
    progress: &dyn GatewayReconciliationProgress,
    launchctl: &dyn Launchctl,
    artifacts: &LaunchdArtifacts,
    cleanup_step: &LifecyclePlanStep,
    service: &str,
    port: u16,
    target: &ReconciliationTarget,
) -> Result<(), String> {
    let observe = || {
        let status = launchctl.status(service)?;
        let observed = observed_incarnation(service, port, &status, launchctl.health(port));
        Ok((
            artifacts.present(cleanup_step.effect(), status.loaded)?,
            observed,
        ))
    };
    cleanup_bootstrap_with(
        progress,
        cleanup_step,
        target,
        observe,
        || launchctl.bootout(service),
        observe,
    )
}

fn cleanup_bootstrap_with(
    progress: &dyn GatewayReconciliationProgress,
    cleanup_step: &LifecyclePlanStep,
    target: &ReconciliationTarget,
    observe_before: impl FnOnce() -> Result<(bool, Option<ReconciliationIncarnation>), String>,
    unload: impl FnOnce() -> Result<(), String>,
    observe_after: impl FnOnce() -> Result<(bool, Option<ReconciliationIncarnation>), String>,
) -> Result<(), String> {
    let (loaded, observed) = observe_before()?;
    let decision = bootstrap_cleanup_decision(loaded, observed.as_ref(), target);
    let cleanup_result = match decision {
        BootstrapCleanupDecision::UnloadExactTarget => unload(),
        BootstrapCleanupDecision::AlreadyAbsent => Ok(()),
        BootstrapCleanupDecision::RefuseReplacement => Err(
            "the label is loaded, but not by the exact healthy bootstrap target; bootout refused"
                .to_owned(),
        ),
    };
    // A refusal is this cleanup's recorded result, not an unrecorded skip.
    let command = match &cleanup_result {
        Ok(()) if decision == BootstrapCleanupDecision::UnloadExactTarget => {
            LifecycleCommandResult::Accepted
        }
        Ok(()) => LifecycleCommandResult::Indeterminate(
            "The exact bootstrap target was already absent during cleanup".into(),
        ),
        Err(error) if decision == BootstrapCleanupDecision::RefuseReplacement => {
            LifecycleCommandResult::Rejected(error.clone())
        }
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };

    progress
        .effect_completed("bootstrap-service", cleanup_step.id(), &command)
        .map_err(|error| format!("{error}; cleanup command result was {command:?}"))?;
    let (refreshed_loaded, refreshed_incarnation) = observe_after()
        .map_err(|error| format!("{error}; cleanup command result was {command:?}"))?;
    progress
        .physical_observed(
            &LifecycleObservationSource::Effect {
                plan_id: "bootstrap-service".into(),
                step_id: cleanup_step.id().into(),
            },
            refreshed_incarnation,
            refreshed_loaded,
        )
        .map_err(|error| format!("{error}; cleanup command result was {command:?}"))?;
    cleanup_result
}

fn run_planned_effect<T>(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    run: impl FnOnce() -> Result<T, String>,
    observe: impl FnOnce() -> Result<(Option<ReconciliationIncarnation>, bool), String>,
) -> Result<T, RegisterFailure> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| RegisterFailure::Physical(error.to_string()))?;
    progress
        .effect_planned(plan_id, &step, &[])
        .map_err(RegisterFailure::Audit)?;
    let result = run();
    let completion = match &result {
        Ok(_) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(audit) = progress.effect_completed(plan_id, step.id(), &completion) {
        return Err(audit_after_physical(audit, &result));
    }
    let (incarnation, target_artifact_present) = observe()?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: step.id().into(),
        },
        incarnation,
        target_artifact_present,
    ) {
        return Err(audit_after_physical(audit, &result));
    }
    result.map_err(RegisterFailure::Physical)
}

/// What a declared cleanup did: removed its artifact, or found it absent.
#[derive(Debug, PartialEq, Eq)]
enum CleanupRun {
    Ran,
    AlreadyAbsent,
}

fn run_planned_effect_with_cleanup<T>(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    cleanup_step: LifecyclePlanStep,
    run: impl FnOnce() -> Result<T, String>,
    cleanup: impl FnOnce() -> Result<CleanupRun, String>,
    observe: impl Fn() -> Result<(Option<ReconciliationIncarnation>, bool), String>,
) -> Result<T, RegisterFailure> {
    let primary =
        LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
            .map_err(|error| RegisterFailure::Physical(error.to_string()))?;
    progress
        .effect_planned(plan_id, &primary, std::slice::from_ref(&cleanup_step))
        .map_err(RegisterFailure::Audit)?;
    let result = run();
    let completion = match &result {
        Ok(_) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    let completion_delivery = progress.effect_completed(plan_id, primary.id(), &completion);
    let mut observed_primary = None;
    let observation_delivery = match &completion_delivery {
        Ok(_) => match observe() {
            Ok((incarnation, target_artifact_present)) => {
                observed_primary = incarnation.clone();
                progress.physical_observed(
                    &LifecycleObservationSource::Effect {
                        plan_id: plan_id.into(),
                        step_id: primary.id().into(),
                    },
                    incarnation,
                    target_artifact_present,
                )
            }
            .map(|_| ()),
            Err(error) => Err(GatewayError::Registration(error)),
        },
        Err(error) => Err(error.clone()),
    };
    // The cleanup runs only once the journal holds the primary's result and
    // observation and that result makes it owed. After a failed delivery it
    // does not run: the journal could not record it, and the next successful
    // registration prunes what is left.
    let owed = completion_delivery.is_ok()
        && observation_delivery.is_ok()
        && cleanup_step
            .predicate()
            .is_due(Some(&completion), observed_primary.as_ref());
    if !owed {
        if let Err(audit) = completion_delivery.and(observation_delivery) {
            return Err(audit_after_physical(audit, &result));
        }
    } else {
        let cleanup_result = cleanup();
        let cleanup_completion = match &cleanup_result {
            Ok(CleanupRun::Ran) => LifecycleCommandResult::Accepted,
            Ok(CleanupRun::AlreadyAbsent) => LifecycleCommandResult::Indeterminate(
                "What the cleanup removes was already absent".into(),
            ),
            Err(error) => LifecycleCommandResult::Failed(error.clone()),
        };
        let cleanup_delivery = progress
            .effect_completed(plan_id, cleanup_step.id(), &cleanup_completion)
            .and_then(|_| {
                let (incarnation, target_artifact_present) =
                    observe().map_err(|error| GatewayError::Registration(error.to_string()))?;
                progress
                    .physical_observed(
                        &LifecycleObservationSource::Effect {
                            plan_id: plan_id.into(),
                            step_id: cleanup_step.id().into(),
                        },
                        incarnation,
                        target_artifact_present,
                    )
                    .map(|_| ())
            });
        if let Err(audit) = cleanup_delivery {
            let cleanup_detail = match &cleanup_result {
                Ok(CleanupRun::Ran) => "; the planned cleanup ran".to_owned(),
                Ok(CleanupRun::AlreadyAbsent) => {
                    "; the planned cleanup found nothing to remove".to_owned()
                }
                Err(error) => format!("; planned cleanup also failed: {error}"),
            };
            return Err(audit_after_physical(
                GatewayError::Registration(format!("{audit}{cleanup_detail}")),
                &result,
            ));
        }
        if let Err(error) = cleanup_result {
            return Err(RegisterFailure::Physical(format!(
                "{}; planned cleanup also failed: {error}",
                result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "gateway audit delivery failed".into())
            )));
        }
    }
    result.map_err(RegisterFailure::Physical)
}

fn audit_after_physical<T, E: Display>(
    audit: GatewayError,
    physical: &Result<T, E>,
) -> RegisterFailure {
    let physical = match physical {
        Ok(_) => GatewayPhysicalResult::Succeeded,
        Err(error) => {
            GatewayPhysicalResult::Failed(Box::new(GatewayError::Registration(error.to_string())))
        }
    };
    RegisterFailure::Audit(GatewayError::Audit {
        audit: audit.to_string(),
        physical: Some(physical),
    })
}

fn artifact_presence(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn prune_planned(
    progress: &dyn GatewayReconciliationProgress,
    artifacts: &LaunchdArtifacts,
    retained: &RetainedRuntimes,
    running: &ReconciliationIncarnation,
) -> Result<(), RegisterFailure> {
    let installations = &artifacts.runtimes;
    let names = match removable_runtime_names(installations, retained) {
        Ok(names) => names,
        Err(error) => {
            eprintln!("[nessa] Could not list staged gateway runtimes; none were removed: {error}");
            return Ok(());
        }
    };
    for name in names {
        let (plan_id, effect) = match name.strip_prefix(".staging-") {
            Some(generation) => (
                format!("remove-staging-runtime-{generation}"),
                LifecycleEffect::RemoveStagingRuntime {
                    generation: generation.into(),
                },
            ),
            None => (
                format!("prune-runtime-{name}"),
                LifecycleEffect::PruneRuntime {
                    fingerprint: name.clone(),
                },
            ),
        };
        let step =
            LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
                .map_err(|error| RegisterFailure::Physical(error.to_string()))?;
        progress
            .effect_planned(&plan_id, &step, &[])
            .map_err(RegisterFailure::Audit)?;
        let removal = prune_runtime(installations, &name);
        let completion = match &removal {
            Ok(()) => LifecycleCommandResult::Accepted,
            Err(error) => LifecycleCommandResult::Failed(error.clone()),
        };
        if let Err(audit) = progress.effect_completed(&plan_id, step.id(), &completion) {
            return Err(audit_after_physical(audit, &removal));
        }
        let artifact_present = artifacts.present(step.effect(), false).map_err(|error| {
            audit_after_physical(
                GatewayError::Registration(format!(
                    "Could not observe pruned gateway runtime {name}: {error}"
                )),
                &removal,
            )
        })?;
        if let Err(audit) = progress.physical_observed(
            &LifecycleObservationSource::Effect {
                plan_id,
                step_id: step.id().into(),
            },
            Some(running.clone()),
            artifact_present,
        ) {
            return Err(audit_after_physical(audit, &removal));
        }
        match removal {
            Ok(()) => eprintln!(
                "[nessa] Removed staged gateway runtime {}",
                installations.join(&name).display()
            ),
            Err(error) => eprintln!(
                "[nessa] Could not remove staged gateway runtime {}: {error}",
                installations.join(&name).display()
            ),
        }
    }
    Ok(())
}

/// The last observation recovery recorded, and whether any settled step had
/// not returned.
struct Settled {
    observation: LifecycleObservation,
    unreturned: bool,
}

/// Closes the attempt as failed, keeping whatever is there. An attempt whose
/// steps had not all returned failed at dispatch; one whose steps had, at
/// observation.
fn close_failed(
    journal: &dyn GatewayReconciliationJournalSession,
    settled: Settled,
    message: &str,
) -> Result<(), GatewayError> {
    retry_journal_delivery(|| {
        journal.physical_outcome(
            &LifecyclePhysicalOutcome::Failed {
                phase: if settled.unreturned {
                    LifecycleFailedPhase::NativeDispatch
                } else {
                    LifecycleFailedPhase::Observation
                },
                message: message.into(),
            },
            Some(&settled.observation),
            ReconciliationCleanupDecision::RetainPrior,
        )
    })
    .map(|_| ())
}

/// Settles every step the domain lists as unsettled, in its order, without
/// running any. The pending step takes `decided` when given; any other
/// unreturned primary is recorded indeterminate because the interrupted
/// attempt may have returned it unrecorded, and an unreturned cleanup as not
/// run by recovery, though the attempt may have run it. Each settled step is observed with `artifact_present` for
/// its effect, and the last observation is returned for the outcome, since no
/// outcome may leave a due step unsettled.
fn settle_without_replay(
    recovery: &GatewayLifecycleRecovery,
    journal: &dyn GatewayReconciliationJournalSession,
    decided: Option<LifecycleCommandResult>,
    observed: Option<ReconciliationIncarnation>,
    artifact_present: impl Fn(&LifecycleEffect) -> Result<bool, String>,
) -> Result<Settled, GatewayError> {
    let pending = recovery.pending_step();
    let mut version = recovery
        .latest_observation()
        .map_or(0, LifecycleObservation::version);
    let mut settled = None;
    let mut unreturned = false;
    for step in recovery.unsettled_steps() {
        if step.completion().is_none() {
            unreturned = true;
            let is_pending = pending.is_some_and(|pending| {
                pending.plan_id() == step.plan_id() && pending.step().id() == step.step().id()
            });
            let is_cleanup = step
                .contingencies()
                .iter()
                .any(|contingency| contingency.id() == step.step().id());
            let command = match (&decided, is_pending, is_cleanup) {
                (Some(decided), true, _) => decided.clone(),
                (_, _, false) => LifecycleCommandResult::Indeterminate(
                    "The step's result was not recorded before recovery".into(),
                ),
                (_, _, true) => LifecycleCommandResult::Indeterminate(
                    "Not run by recovery; whether the interrupted attempt ran it is not recorded"
                        .into(),
                ),
            };
            retry_journal_delivery(|| {
                journal.effect_completion(step.plan_id(), step.step().id(), &command)
            })?;
        }
        version = version.saturating_add(1);
        let observation = LifecycleObservation::new(
            version,
            observed.clone(),
            artifact_present(step.step().effect()).map_err(GatewayError::Registration)?,
        );
        retry_journal_delivery(|| journal.observation(&step.source(), &observation))?;
        settled = Some(observation);
    }
    let observation = settled.ok_or_else(|| {
        GatewayError::Registration("The unresolved gateway plan has nothing to settle".into())
    })?;
    Ok(Settled {
        observation,
        unreturned,
    })
}

/// The prune a first staging declares for the runtime it publishes, owed
/// only when staging did not succeed.
fn staging_cleanup(fingerprint: &str) -> Result<LifecyclePlanStep, String> {
    LifecyclePlanStep::new(
        "remove-published-runtime".into(),
        LifecycleEffect::PruneRuntime {
            fingerprint: fingerprint.into(),
        },
        LifecycleEffectPredicate::PrimaryNotAccepted,
    )
    .map_err(|error| error.to_string())
}

/// The bootout a bootstrap declares, owed only when the bootstrap did not
/// succeed: after a success nothing is left pending for the next step.
fn bootstrap_cleanup(service: &str) -> Result<LifecyclePlanStep, String> {
    LifecyclePlanStep::new(
        "unload-bootstrapped-service".into(),
        LifecycleEffect::UnloadService {
            service: service.into(),
        },
        LifecycleEffectPredicate::PrimaryNotAccepted,
    )
    .map_err(|error| error.to_string())
}

/// Unloads the service as one journaled plan, observing what launchd shows
/// afterwards. The stale, legacy and unavailable registrations all leave this
/// way.
fn unload_service(
    progress: &dyn GatewayReconciliationProgress,
    launchctl: &dyn Launchctl,
    artifacts: &LaunchdArtifacts,
    plan_id: &str,
    service: &str,
    port: u16,
) -> Result<(), RegisterFailure> {
    let effect = LifecycleEffect::UnloadService {
        service: service.into(),
    };
    run_planned_effect(
        progress,
        plan_id,
        effect.clone(),
        || launchctl.bootout(service),
        || {
            let status = launchctl.status(service)?;
            Ok((
                observed_incarnation(service, port, &status, launchctl.health(port)),
                artifacts.present(&effect, status.loaded)?,
            ))
        },
    )
}

/// What one bootstrap step needs to know about its registration.
struct BootstrapRequest<'a> {
    domain: &'a str,
    label: &'a str,
    service: &'a str,
    artifacts: &'a LaunchdArtifacts,
    port: u16,
    target: &'a ReconciliationTarget,
}

/// Bootstraps the published definition as one journaled plan: its result and
/// observation are recorded, and a bootout it owes runs only once the journal
/// holds a refused result. Every launchd call goes through `launchctl`.
fn bootstrap_service(
    progress: &dyn GatewayReconciliationProgress,
    launchctl: &dyn Launchctl,
    disabled_services: &dyn DisabledServiceStatus,
    request: BootstrapRequest<'_>,
) -> Result<(), RegisterFailure> {
    let BootstrapRequest {
        domain,
        label,
        service,
        artifacts,
        port,
        target,
    } = request;
    let bootstrap_step = LifecyclePlanStep::new(
        "primary".into(),
        LifecycleEffect::BootstrapService {
            target: target.clone(),
        },
        LifecycleEffectPredicate::Always,
    )
    .map_err(|error| error.to_string())?;
    let bootstrap_cleanup = bootstrap_cleanup(service)?;
    progress
        .effect_planned(
            "bootstrap-service",
            &bootstrap_step,
            std::slice::from_ref(&bootstrap_cleanup),
        )
        .map_err(RegisterFailure::Audit)?;
    let bootstrap = run_bootstrap(progress, || launchctl.bootstrap(domain, &artifacts.plist))
        .map_err(|error| BootstrapFailure::CouldNotRun(error.to_string()))
        .and_then(bootstrap_result);
    let bootstrap_completion = match &bootstrap {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Rejected(error.to_string()),
    };
    if let Err(audit) = progress.effect_completed(
        "bootstrap-service",
        bootstrap_step.id(),
        &bootstrap_completion,
    ) {
        return Err(audit_after_physical(audit, &bootstrap));
    }
    let status_after_bootstrap = launchctl.status(service)?;
    let observed_after_bootstrap = observed_incarnation(
        service,
        port,
        &status_after_bootstrap,
        launchctl.health(port),
    );
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: "bootstrap-service".into(),
            step_id: "primary".into(),
        },
        observed_after_bootstrap.clone(),
        artifacts.present(bootstrap_step.effect(), status_after_bootstrap.loaded)?,
    ) {
        return Err(audit_after_physical(audit, &bootstrap));
    }
    // A bootstrap launchd refused owes its bootout, run and recorded before
    // the failure is reported; one it refuses is recorded as refused.
    let cleanup = if bootstrap_cleanup.predicate().is_due(
        Some(&bootstrap_completion),
        observed_after_bootstrap.as_ref(),
    ) {
        cleanup_bootstrap(
            progress,
            launchctl,
            artifacts,
            &bootstrap_cleanup,
            service,
            port,
            target,
        )
    } else {
        Ok(())
    };
    let finished = finish_bootstrap(
        bootstrap,
        || launchctl.status(service).map(|status| status.loaded),
        || disabled_services.is_disabled(domain, label),
    );
    match (finished, cleanup) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(cleanup)) => {
            return Err(format!("predeclared bootstrap cleanup: {cleanup}").into())
        }
        (Err(failure), Ok(())) => return Err(failure.into()),
        (Err(failure), Err(cleanup)) => {
            return Err(format!("{failure}; predeclared bootstrap cleanup: {cleanup}").into())
        }
    }
    bootstrap_succeeded(progress);
    Ok(())
}

fn register(
    host: &Launchd,
    runtime: &Path,
    stage: &str,
    agent_path: Option<&SearchPath>,
    attempt: &GatewayReconciliationAttempt,
    progress: &dyn GatewayReconciliationProgress,
) -> Result<ReconciledGateway, RegisterFailure> {
    let configuration = &host.configuration;
    let launchctl = host.launchctl.as_ref();
    let home = host.home.as_path();
    let location = runtime.to_string_lossy();
    if location.starts_with("/Volumes/") || location.contains("/AppTranslocation/") {
        return Err("Move Nessa to Applications before starting its background service".into());
    }
    if configuration.stage() != stage {
        return Err("Gateway service configuration stage changed".into());
    }
    let instance = configuration.instance();
    let base = configuration.data_root();
    // One table decides where a stage listens. A packaged build registers the
    // `prod` service and keeps 7420; a stage with no entry gets no service at
    // all rather than silently taking the product's socket.
    let port = configuration.port();
    let data = data_directory_path(base, stage, instance)?;
    let log = data.join("logs/gateway.log");
    let label = service_label(stage, instance);
    let lock_directory = home
        .join("Library/Application Support/Nessa/gateway-locks")
        .join(&label);
    nessa_local_storage::create_directory(&lock_directory).map_err(|e| e.to_string())?;
    let _lock = lock_namespace(&lock_directory)?;
    let uid = unsafe { libc::getuid() };
    let domain = format!("gui/{uid}");
    let service = format!("{domain}/{label}");
    let status = launchctl.status(&service)?;
    let loaded = status.loaded;
    let loaded_pid = status.pid;
    let process_identity_known = status.process_identity_known;
    let fingerprint = runtime_fingerprint(runtime)?;
    let artifacts = LaunchdArtifacts::for_label(home, &label);
    let installations = artifacts.runtimes.clone();
    let planned_runtime = installations.join(&fingerprint);
    let staged_effect = LifecycleEffect::StageRuntime {
        fingerprint: fingerprint.clone(),
    };
    let runtime_for_definition = planned_runtime.as_path();
    let arguments = launch_settings(runtime_for_definition);
    let agents = home.join("Library/LaunchAgents");
    let path = artifacts.plist.clone();
    let installed = read_definition(&path).ok();
    let agent_path = registered_agent_path(agent_path, installed.as_ref(), runtime_for_definition);
    let environment = service_environment(configuration, home, &agent_path, &fingerprint);
    // `KeepAlive: true` restarts the service whatever it did, so a server that
    // could never start was relaunched every five seconds for as long as the
    // user was logged in. `SuccessfulExit: false` restarts it only when the
    // process ended unsuccessfully, which still covers every crash and every
    // failure the server thinks retrying can fix — those keep their non-zero
    // exit code from `protocol/defaults/gateway-exit-codes.json`. A failure
    // retrying cannot fix exits zero on purpose and is left alone, with its
    // reason in `logs/gateway-startup-failure.json` for this host to read.
    //
    // Verified against launchd on macOS 26 (Darwin 25.6) rather than assumed:
    // a job exiting 0 under this dictionary runs once and stops, one exiting 1
    // is respawned every ThrottleInterval, and one killed by SIGSEGV is
    // respawned too — launchd prints no `last exit code` for that at all, only
    // the terminating signal.
    let mut definition = serde_json::json!({
        "Label":label, "ProgramArguments":arguments,
        "WorkingDirectory":data, "EnvironmentVariables": environment, "RunAtLoad":true,
        "KeepAlive":{"SuccessfulExit":false},
        "ThrottleInterval":5,"ExitTimeOut":30,"ProcessType":"Background",
        "StandardOutPath":log,"StandardErrorPath":log
    });
    let installed_data = installed
        .as_ref()
        .and_then(|definition| definition.get("WorkingDirectory"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    // Where the loaded registration writes its log, and beside it the record it
    // leaves when it stops for good. A definition we cannot read leaves only
    // this host's own namespace to look in.
    let installed_logs = installed_data
        .clone()
        .unwrap_or_else(|| data.clone())
        .join("logs");
    let fence = match installed_data.as_deref() {
        Some(data) => read_retirement_evidence(data)?,
        None => None,
    };
    let running = if loaded { launchctl.health(port) } else { None };
    let pending = match (&running, installed_data.as_deref()) {
        (Some(Health::Managed(runtime)), Some(data)) => read_pending_retirement(data, runtime)?,
        _ => None,
    };
    let mut generation = service_generation(&definition, installed.as_ref(), fence.as_ref())?;
    if pending
        .as_ref()
        .is_some_and(|pending| pending.matches(&fingerprint, &generation))
    {
        generation = service_generation(&definition, None, pending.as_ref())?;
    }
    definition["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] =
        Value::String(generation.clone());
    let running_fenced = matches!(&running, Some(Health::Managed(runtime)) if fence.as_ref().is_some_and(|fence| fence.matches(&runtime.fingerprint, &runtime.generation)) || pending.as_ref().is_some_and(|pending| pending.matches(&runtime.fingerprint, &runtime.generation)));
    let listener = if matches!(running, Some(Health::Legacy)) {
        legacy_listener_pid(port)
    } else {
        None
    };
    let registration = if loaded {
        Registration::Loaded
    } else {
        Registration::Unloaded
    };
    let state = classify(
        registration,
        loaded_pid,
        loaded && !running_fenced && service_matches(&path, &definition),
        running.clone(),
        (&fingerprint, &generation),
        TcpStream::connect_timeout(&address(port), Duration::from_millis(200)).is_ok(),
        listener,
    );
    let before = match &running {
        Some(Health::Managed(runtime)) => Some(ReconciledGateway::new(
            service.clone(),
            runtime.fingerprint.clone(),
            runtime.instance.clone(),
            runtime.generation.clone(),
            runtime.pid,
            port,
        )),
        _ => None,
    };
    let before = before
        .map(|gateway| gateway.audit_identity())
        .transpose()
        .map_err(|error| error.to_string())?;
    let target =
        ReconciliationTarget::new(service.clone(), fingerprint.clone(), generation.clone())
            .map_err(|error| error.to_string())?;
    let intent = GatewayReconciliationIntent::new(attempt.clone(), target.clone(), before.clone())
        .map_err(|error| error.to_string())?;
    progress
        .intent_admitted(intent)
        .map_err(RegisterFailure::Audit)?;
    let staged_runtime_existed = fs::symlink_metadata(&planned_runtime).is_ok();
    let _staged_runtime = if staged_runtime_existed {
        // The exact runtime is already published: this plan reuses it, and
        // its name says so in the journal.
        run_planned_effect(
            progress,
            "reuse-staged-runtime",
            LifecycleEffect::StageRuntime {
                fingerprint: fingerprint.clone(),
            },
            || {
                prepare_data_directory(base, stage, instance)?;
                stage_runtime_cached(
                    runtime,
                    &installations,
                    &fingerprint,
                    &host.validated_runtimes,
                )
            },
            || {
                let status = launchctl.status(&service)?;
                Ok((
                    observed_incarnation(&service, port, &status, launchctl.health(port)),
                    artifacts.present(&staged_effect, status.loaded)?,
                ))
            },
        )?
    } else {
        let cleanup_staged_runtime = staging_cleanup(&fingerprint)?;
        run_planned_effect_with_cleanup(
            progress,
            "stage-runtime",
            LifecycleEffect::StageRuntime {
                fingerprint: fingerprint.clone(),
            },
            cleanup_staged_runtime,
            || {
                prepare_data_directory(base, stage, instance)?;
                stage_runtime_cached(
                    runtime,
                    &installations,
                    &fingerprint,
                    &host.validated_runtimes,
                )
            },
            || {
                // Staging that failed before publishing left nothing to prune.
                if artifacts.present(&staged_effect, false)? {
                    prune_runtime(&installations, &fingerprint).map(|()| CleanupRun::Ran)
                } else {
                    Ok(CleanupRun::AlreadyAbsent)
                }
            },
            || {
                let status = launchctl.status(&service)?;
                Ok((
                    observed_incarnation(&service, port, &status, launchctl.health(port)),
                    artifacts.present(&staged_effect, status.loaded)?,
                ))
            },
        )?
    };
    match state {
        ServiceState::ManagedCurrent(running) => {
            // The loaded service already advertises the staged runtime, so the
            // versions nothing can be running are known here too. Collecting
            // only after a replacement would leave an ordinary launch holding
            // whatever the last update left behind until the next one.
            let gateway = ReconciledGateway::new(
                service.clone(),
                running.fingerprint.clone(),
                running.instance.clone(),
                running.generation.clone(),
                running.pid,
                port,
            );
            prune_planned(
                progress,
                &artifacts,
                &retained_runtimes(
                    &fingerprint,
                    &running.fingerprint,
                    pending.as_ref(),
                    fence.as_ref(),
                ),
                &gateway.audit_identity().map_err(RegisterFailure::Audit)?,
            )?;
            return Ok(gateway);
        }
        ServiceState::ManagedStale(running) => {
            progress.step_started(StartupStep::Replacing);
            progress.readiness_invalidated();
            let old_definition = read_definition(&path)?;
            let old_data = old_definition
                .get("WorkingDirectory")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or("Loaded definition has no absolute data namespace")?;
            let retirement = LifecycleEffect::RequestRetirement {
                incarnation: ReconciledGateway::new(
                    service.clone(),
                    running.fingerprint.clone(),
                    running.instance.clone(),
                    running.generation.clone(),
                    running.pid,
                    port,
                )
                .audit_identity()
                .map_err(RegisterFailure::Audit)?,
            };
            run_planned_effect(
                progress,
                "retire-current-runtime",
                retirement.clone(),
                || {
                    retire(
                        launchctl,
                        &old_data,
                        control::Retirement {
                            service: &service,
                            target: &fingerprint,
                            running: &running.fingerprint,
                            instance: &running.instance,
                            running_generation: &running.generation,
                            target_generation: &generation,
                        },
                    )
                },
                || {
                    let status = launchctl.status(&service)?;
                    Ok((
                        observed_incarnation(&service, port, &status, launchctl.health(port)),
                        artifacts.present(&retirement, status.loaded)?,
                    ))
                },
            )?;
            progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
            unload_service(
                progress,
                launchctl,
                &artifacts,
                "unload-stale-service",
                &service,
                port,
            )?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
        }
        ServiceState::LegacyExactService => {
            // This exact pre-upgrade registration has no retirement protocol.
            // SIGTERM cancels its active agents; never send it SIGUSR2.
            eprintln!("[nessa] Retiring legacy gateway {service}; active agents will be stopped by server shutdown");
            progress.step_started(StartupStep::Replacing);
            progress.readiness_invalidated();
            unload_service(
                progress,
                launchctl,
                &artifacts,
                "unload-legacy-service",
                &service,
                port,
            )?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
        }
        ServiceState::ForeignPort => {
            return Err(format!(
                "Port {port} is occupied by an unmanaged process; no service was stopped"
            )
            .into())
        }
        ServiceState::UnavailableLoadedService => {
            // A service that gave up is loaded with no process and will not be
            // restarted by launchd, so nothing but this host will ever start it
            // again — and whatever it gave up over may well have been fixed
            // since. Its own record, for the registration that is actually
            // installed, is what says so.
            let recorded = startup::recorded_failure(&installed_logs).filter(|record| {
                installed_generation(installed.as_ref())
                    .is_some_and(|generation| record.belongs_to(generation))
            });
            if !process_identity_known {
                return Err(unreadable_process_identity().into());
            }
            if !gave_up_retry(loaded_pid, process_identity_known, recorded.is_some()) {
                return Err(unavailable_service(recorded.as_ref(), port).into());
            }
            if let Some(recorded) = &recorded {
                // What is being replaced, why, and on whose say-so, in the log
                // of the process doing it. The registration has no running
                // process to retire and no conversations to stop.
                eprintln!(
                    "[nessa] Replacing gateway {service}, which launchd will not start again: {}",
                    recorded.describe()
                );
            }
            progress.step_started(StartupStep::Replacing);
            progress.readiness_invalidated();
            unload_service(
                progress,
                launchctl,
                &artifacts,
                "unload-unavailable-service",
                &service,
                port,
            )?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
            if recorded.is_some() {
                startup::forget_recorded_failure(&installed_logs);
            }
        }
        ServiceState::Unloaded => {
            progress.readiness_invalidated();
        }
    }
    let installation = (|| -> Result<ManagedRuntime, RegisterFailure> {
        nessa_local_storage::create_private_directory_path(&agents)
            .map_err(|error| error.to_string())?;
        let agents_directory =
            PrivateDirectory::open_path(&agents, &agents).map_err(|error| error.to_string())?;
        let logs = log.parent().ok_or("invalid log directory")?;
        nessa_local_storage::create_directory(logs).map_err(|e| e.to_string())?;
        // Reserve the log privately before launchd opens it.
        let _ =
            nessa_local_storage::open(&log, OpenMode::OpenOrCreate).map_err(|e| e.to_string())?;
        let definition_json = serde_json::to_vec(&definition).map_err(|error| error.to_string())?;
        let definition_xml = convert_definition_to_xml(&definition_json)?;
        let mut next = agents_directory
            .reserve_temp()
            .map_err(|error| error.to_string())?;
        next.as_file_mut()
            .write_all(&definition_xml)
            .map_err(|error| error.to_string())?;
        next.as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        let destination = format!("{label}.plist");
        let publication = LifecycleEffect::PublishServiceDefinition {
            target: target.clone(),
        };
        run_planned_effect(
            progress,
            "publish-service-definition",
            publication.clone(),
            || {
                publish_definition(
                    progress,
                    || {
                        next.replace(std::ffi::OsStr::new(&destination))
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    },
                    || {
                        nessa_local_storage::sync_directory(&agents)
                            .map_err(|error| error.to_string())
                    },
                )
            },
            || {
                let status = launchctl.status(&service)?;
                Ok((
                    observed_incarnation(&service, port, &status, launchctl.health(port)),
                    artifacts.present(&publication, status.loaded)?,
                ))
            },
        )?;
        progress.step_started(StartupStep::Launching);
        bootstrap_service(
            progress,
            launchctl,
            host.disabled_services.as_ref(),
            BootstrapRequest {
                domain: &domain,
                label: &label,
                service: &service,
                artifacts: &artifacts,
                port,
                target: &target,
            },
        )?;
        let readiness_step = LifecyclePlanStep::new(
            "primary".into(),
            LifecycleEffect::AdoptReadyIncarnation {
                target: target.clone(),
            },
            LifecycleEffectPredicate::Always,
        )
        .map_err(|error| error.to_string())?;
        progress
            .effect_planned("adopt-ready-incarnation", &readiness_step, &[])
            .map_err(RegisterFailure::Audit)?;
        let running =
            match wait_fingerprint(launchctl, &service, (&fingerprint, &generation), port, &log) {
                Ok(running) => {
                    progress
                        .effect_completed(
                            "adopt-ready-incarnation",
                            readiness_step.id(),
                            &LifecycleCommandResult::Accepted,
                        )
                        .map_err(RegisterFailure::Audit)?;
                    running
                }
                Err(error) => {
                    progress
                        .effect_completed(
                            "adopt-ready-incarnation",
                            readiness_step.id(),
                            &LifecycleCommandResult::Failed(format!("{error:?}")),
                        )
                        .map_err(RegisterFailure::Audit)?;
                    let status = launchctl.status(&service)?;
                    let observed =
                        observed_incarnation(&service, port, &status, launchctl.health(port));
                    progress
                        .physical_observed(
                            &LifecycleObservationSource::Effect {
                                plan_id: "adopt-ready-incarnation".into(),
                                step_id: "primary".into(),
                            },
                            observed,
                            artifacts.present(readiness_step.effect(), status.loaded)?,
                        )
                        .map_err(RegisterFailure::Audit)?;
                    return Err(error.into());
                }
            };
        let running_identity = ReconciledGateway::new(
            service.clone(),
            running.fingerprint.clone(),
            running.instance.clone(),
            running.generation.clone(),
            running.pid,
            port,
        )
        .audit_identity()
        .map_err(|error| error.to_string())?;
        progress
            .physical_observed(
                &LifecycleObservationSource::Effect {
                    plan_id: "adopt-ready-incarnation".into(),
                    step_id: "primary".into(),
                },
                Some(running_identity),
                // The ready gateway is running, so launchd has its label loaded.
                artifacts.present(readiness_step.effect(), true)?,
            )
            .map_err(RegisterFailure::Audit)?;
        Ok(running)
    })();
    let running = installation?;
    // An installation that failed leaves the old registration, and possibly an
    // old process, alive for a retry.
    let retained = retained_runtimes(
        &fingerprint,
        &running.fingerprint,
        pending.as_ref(),
        fence.as_ref(),
    );
    let gateway = ReconciledGateway::new(
        service,
        running.fingerprint,
        running.instance,
        running.generation,
        running.pid,
        port,
    );
    prune_planned(
        progress,
        &artifacts,
        &retained,
        &gateway.audit_identity().map_err(RegisterFailure::Audit)?,
    )?;
    Ok(gateway)
}

fn service_environment(
    configuration: &ServiceConfiguration,
    home: &Path,
    agent_path: &SearchPath,
    fingerprint: &str,
) -> serde_json::Map<String, Value> {
    let mut environment = serde_json::Map::new();
    for (key, value) in [
        ("HOME", home.to_string_lossy().into_owned()),
        ("NESSA_STAGE", configuration.stage().to_owned()),
        ("NESSA_HOST", "127.0.0.1".into()),
        ("NESSA_PORT", configuration.port().to_string()),
        ("PATH", SearchPath::system().as_str().to_owned()),
        ("NESSA_AGENT_PATH", agent_path.as_str().to_owned()),
        ("NESSA_RUNTIME_FINGERPRINT", fingerprint.to_owned()),
        (
            "NESSA_DATA_DIR",
            configuration.data_root().to_string_lossy().into_owned(),
        ),
    ] {
        environment.insert(key.into(), value.into());
    }
    if let Some(instance) = configuration.instance() {
        environment.insert("NESSA_INSTANCE".into(), instance.into());
    }
    if let Some(directory) = configuration.claude_config_directory() {
        environment.insert(
            "CLAUDE_CONFIG_DIR".into(),
            directory.to_string_lossy().into_owned().into(),
        );
    }
    environment
}

fn service_label(stage: &str, instance: Option<&str>) -> String {
    format!(
        "so.nessa.gateway.{stage}{}",
        instance
            .map(|value| format!(".{value}"))
            .unwrap_or_default()
    )
}

fn provider_configuration_changed(configuration: &ServiceConfiguration, home: &Path) -> bool {
    let path = home.join("Library/LaunchAgents").join(format!(
        "{}.plist",
        service_label(configuration.stage(), configuration.instance())
    ));
    let Ok(definition) = read_definition(&path) else {
        return false;
    };
    let registered = definition
        .get("EnvironmentVariables")
        .and_then(|environment| environment.get("CLAUDE_CONFIG_DIR"))
        .and_then(Value::as_str)
        .map(Path::new);
    registered != configuration.claude_config_directory()
}

enum RegisterFailure {
    Physical(String),
    Audit(GatewayError),
}

impl From<String> for RegisterFailure {
    fn from(message: String) -> Self {
        Self::Physical(message)
    }
}

impl From<&str> for RegisterFailure {
    fn from(message: &str) -> Self {
        Self::Physical(message.into())
    }
}
impl From<InstallFailure> for RegisterFailure {
    fn from(failure: InstallFailure) -> Self {
        let message = forward_recovery::<()>(Err(failure))
            .expect_err("install failure cannot become a successful value");
        Self::Physical(message)
    }
}
fn prepare_data_directory(
    trusted_base: &Path,
    stage: &str,
    instance: Option<&str>,
) -> Result<PathBuf, String> {
    let destination = data_directory_path(trusted_base, stage, instance)?;
    nessa_local_storage::create_directory(trusted_base).map_err(|error| error.to_string())?;
    let Ok(relative) = destination.strip_prefix(trusted_base) else {
        return Err("Gateway data directory escaped its trusted base".into());
    };
    if relative.as_os_str().is_empty() {
        return Ok(destination);
    }
    nessa_local_storage::create_directory_beneath(trusted_base, relative)
        .map_err(|error| error.to_string())?;
    Ok(destination)
}

fn data_directory_path(
    trusted_base: &Path,
    stage: &str,
    instance: Option<&str>,
) -> Result<PathBuf, String> {
    let mut relative = PathBuf::new();
    if stage != "prod" {
        relative.push(stage);
    }
    if let Some(instance) = instance {
        relative.push("instances");
        relative.push(instance);
    }
    if relative.as_os_str().is_empty() {
        return Ok(trusted_base.to_path_buf());
    }
    Ok(trusted_base.join(relative))
}
#[derive(Deserialize)]
struct RuntimeManifest {
    fingerprint: String,
}
fn runtime_fingerprint(runtime: &Path) -> Result<String, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(runtime.join("manifest.json"))
        .map_err(|_| "Missing or unsafe runtime manifest")?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Runtime manifest must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65536 {
        return Err("Runtime manifest exceeds limit".into());
    }
    let manifest: RuntimeManifest =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid runtime manifest")?;
    if manifest.fingerprint.len() != 64
        || !manifest
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("Invalid runtime fingerprint".into());
    }
    Ok(manifest.fingerprint)
}
/// The search path this registration will give the agent.
///
/// Three sources, in order of authority:
///
/// 1. what an installed definition for this exact runtime already says. A shell
///    profile is launch context, not durable service configuration, so changing
///    it cannot retire a healthy same-runtime gateway;
/// 2. for a new runtime, what the login shell said this launch;
/// 3. failing that — no shell, no prior registration — the system path, which
///    is a working `PATH` with none of the user's tools on it.
///
/// The staged runtime comes out of all three. It is where Nessa's own `node`
/// lives, and an agent that finds that one by name is running a Node the user
/// did not choose.
fn registered_agent_path(
    resolved: Option<&SearchPath>,
    installed: Option<&Value>,
    runtime: &Path,
) -> SearchPath {
    let registered = installed
        .filter(|definition| definition.get("ProgramArguments") == Some(&launch_settings(runtime)))
        .and_then(|definition| definition.get("EnvironmentVariables"))
        .and_then(|environment| environment.get("NESSA_AGENT_PATH"))
        .and_then(Value::as_str)
        .and_then(|path| SearchPath::parse(path).ok());
    registered
        .or_else(|| resolved.cloned())
        .and_then(|path| path.excluding(runtime))
        .unwrap_or_else(SearchPath::system)
}
fn service_matches(path: &Path, expected: &Value) -> bool {
    read_definition(path).is_ok_and(|actual| actual == *expected)
}

fn convert_definition_to_xml(definition: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-", "--", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Could not open plist converter input")?
        .write_all(definition)
        .map_err(|error| error.to_string())?;
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            "Could not write gateway service definition".into()
        } else {
            format!("Could not write gateway service definition: {detail}")
        });
    }
    Ok(output.stdout)
}

fn read_definition(path: &Path) -> Result<Value, String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Cannot read registered gateway definition".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}
/// Where a gateway on `port` answers. Loopback only; the service never binds wider.
fn address(port: u16) -> SocketAddr {
    ([127, 0, 0, 1], port).into()
}
/// Whether a loaded service that gave up may be replaced with a fresh attempt.
///
/// launchd will not start it again — that is what giving up means — so it would
/// otherwise stay loaded and dead forever, including after someone has repaired
/// the thing it gave up over. Booting out a registration with no process of its
/// own destroys nothing; what has to be established is that there is no process,
/// unambiguously, and that the record is this registration's own. The same
/// Its authority is the gateway's own startup-failure record, from different evidence.
fn gave_up_retry(
    loaded_pid: Option<u32>,
    process_identity_known: bool,
    recorded_for_this_registration: bool,
) -> bool {
    process_identity_known && loaded_pid.is_none() && recorded_for_this_registration
}

/// The generation the installed definition registered, if it names one.
fn installed_generation(installed: Option<&Value>) -> Option<&str> {
    installed?
        .get("EnvironmentVariables")?
        .get("NESSA_SERVICE_GENERATION")?
        .as_str()
}

/// What a loaded service that is not answering is reported as. A gateway that
/// said why it stopped is quoted; "no valid health response" is what is left
/// when nothing said anything.
fn unavailable_service(recorded: Option<&startup::RecordedFailure>, port: u16) -> String {
    match recorded.and_then(|record| record.sentence(port)) {
        Some(cause) => {
            format!(
                "Nessa's background service is not starting: {cause} The service was preserved."
            )
        }
        None => "Loaded gateway has no valid health response; service was preserved".into(),
    }
}

fn unreadable_process_identity() -> &'static str {
    "Nessa could not read launchd's process identity, so the background service was preserved rather than risking a live gateway. Quit Nessa and open it again to retry; unresolved lifecycle evidence must be settled before any replacement."
}

#[derive(Debug, PartialEq, Eq)]
enum BootstrapFailure {
    CouldNotRun(String),
    Refused { status: Option<i32>, detail: String },
}

impl Display for BootstrapFailure {
    fn fmt(&self, out: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CouldNotRun(error) => write!(out, "Could not run launchctl bootstrap: {error}"),
            Self::Refused { detail, .. } => write!(out, "Could not register gateway: {detail}"),
        }
    }
}

fn bootstrap_result(output: Output) -> Result<(), BootstrapFailure> {
    if output.status.success() {
        return Ok(());
    }
    Err(BootstrapFailure::Refused {
        status: output.status.code(),
        detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

pub(super) trait DisabledServiceStatus: Send + Sync {
    fn is_disabled(&self, domain: &str, label: &str) -> Result<bool, String>;
}

pub(super) struct LaunchctlDisabledServiceStatus;

impl DisabledServiceStatus for LaunchctlDisabledServiceStatus {
    fn is_disabled(&self, domain: &str, label: &str) -> Result<bool, String> {
        let output = bounded_output(
            Command::new("/bin/launchctl")
                .args(["print-disabled", domain])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            Duration::from_secs(2),
        )?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        disabled_service(&String::from_utf8_lossy(&output.stdout), label)
    }
}

fn disabled_service(output: &str, label: &str) -> Result<bool, String> {
    let output = output.trim();
    if !output.starts_with("disabled services = {") || !output.ends_with('}') {
        return Err("launchctl returned malformed disabled-service status".into());
    }
    let expected = format!("\"{label}\" => disabled");
    let enabled = format!("\"{label}\" => enabled");
    for line in output.lines().map(str::trim) {
        if line == expected {
            return Ok(true);
        }
        if line == enabled {
            return Ok(false);
        }
        if line.starts_with(&format!("\"{label}\" =>")) {
            return Err("launchctl returned an unknown disabled-service value".into());
        }
    }
    Ok(false)
}

fn finish_bootstrap(
    bootstrap: Result<(), BootstrapFailure>,
    loaded_after_failure: impl FnOnce() -> Result<bool, String>,
    disabled_after_failure: impl FnOnce() -> Result<bool, String>,
) -> Result<(), String> {
    let Err(error) = bootstrap else {
        return Ok(());
    };
    match loaded_after_failure() {
        Ok(false) => {
            let disabled = if matches!(
                &error,
                BootstrapFailure::Refused {
                    status: Some(5),
                    ..
                }
            ) {
                disabled_after_failure()
            } else {
                Ok(false)
            };
            let blocked = matches!(disabled, Ok(true));
            let diagnosis = disabled
                .err()
                .map(|diagnosis| format!("; disabled-item diagnosis also failed: {diagnosis}"))
                .unwrap_or_default();
            if blocked {
                return Err(
                    "macOS disabled Nessa's background item. Open System Settings → General → Login Items and allow Nessa to run in the background, then choose Retry."
                        .into(),
                );
            }
            Err(format!(
                "{error}; launchd reports no loaded service{diagnosis}"
            ))
        }
        Ok(true) => Err(format!("{error}; launchd reports a loaded service")),
        Err(status) => Err(format!(
            "{error}; loaded service state could not be verified: {status}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        artifact_presence, bootstrap_cleanup_decision, bootstrap_recovery_decision,
        bootstrap_succeeded, cleanup_bootstrap_with, disabled_service, finish_bootstrap,
        gave_up_retry, installed_generation, matches_reconciled_gateway, prepare_data_directory,
        publish_definition, recovery_probe_port, registered_agent_path, retire_then_unload,
        run_bootstrap, run_planned_effect_with_cleanup, runtime_fingerprint, service_environment,
        service_matches, startup, unavailable_service, unreadable_process_identity,
        BootstrapCleanupDecision, BootstrapFailure, BootstrapRecoveryDecision, SearchPath,
    };
    use crate::gateway::application::{
        GatewayError, GatewayReconciliationIntent, GatewayReconciliationProgress,
        ReconciledGateway, ReconciliationHistoryFact,
    };
    use crate::gateway::domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, LifecycleEffect, LifecycleEffectPredicate,
        LifecycleObservation, LifecycleObservationSource, LifecyclePlanStep, LifecycleRecordKind,
        ReconciliationCorrelation, ReconciliationIncarnation, ReconciliationTarget,
        ServiceConfiguration,
    };
    use crate::gateway::infrastructure::macos::control::{Health, ManagedRuntime, ServiceStatus};
    use crate::gateway::infrastructure::macos::staging::launch_settings;
    use crate::gateway::infrastructure::macos::startup::LastExit;
    use serde_json::{json, Value};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    #[derive(Default)]
    struct RecordingProgress(Mutex<Vec<ReconciliationHistoryFact>>);

    impl GatewayReconciliationProgress for RecordingProgress {
        fn readiness_invalidated(&self) {}

        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }

        fn history_observed(&self, fact: ReconciliationHistoryFact) {
            self.0.lock().unwrap().push(fact);
        }

        fn effect_planned(
            &self,
            _: &str,
            _: &LifecyclePlanStep,
            _: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(test_receipt(1, LifecycleRecordKind::EffectPlan))
        }

        fn effect_completed(
            &self,
            _: &str,
            _: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(test_receipt(2, LifecycleRecordKind::EffectCompletion))
        }

        fn physical_observed(
            &self,
            _: &LifecycleObservationSource,
            incarnation: Option<ReconciliationIncarnation>,
            target_artifact_present: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            Ok(LifecycleObservation::new(
                1,
                incarnation,
                target_artifact_present,
            ))
        }
    }

    struct BootstrapCleanupProgress {
        events: Arc<Mutex<Vec<String>>>,
        completion_failure: bool,
        observation_failure: bool,
    }

    impl GatewayReconciliationProgress for BootstrapCleanupProgress {
        fn readiness_invalidated(&self) {}

        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }

        fn history_observed(&self, _: ReconciliationHistoryFact) {}

        fn effect_planned(
            &self,
            _: &str,
            _: &LifecyclePlanStep,
            _: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(test_receipt(1, LifecycleRecordKind::EffectPlan))
        }

        fn effect_completed(
            &self,
            _: &str,
            step_id: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.events
                .lock()
                .unwrap()
                .push(format!("complete:{step_id}"));
            if self.completion_failure {
                Err(GatewayError::Registration(
                    "cleanup completion unavailable".into(),
                ))
            } else {
                Ok(test_receipt(2, LifecycleRecordKind::EffectCompletion))
            }
        }

        fn physical_observed(
            &self,
            _: &LifecycleObservationSource,
            incarnation: Option<ReconciliationIncarnation>,
            target_artifact_present: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            self.events.lock().unwrap().push("observe-cleanup".into());
            if self.observation_failure {
                Err(GatewayError::Registration(
                    "cleanup observation unavailable".into(),
                ))
            } else {
                Ok(LifecycleObservation::new(
                    2,
                    incarnation,
                    target_artifact_present,
                ))
            }
        }
    }

    fn bootstrap_cleanup_step() -> LifecyclePlanStep {
        super::bootstrap_cleanup("gui/501/so.nessa.gateway.prod").unwrap()
    }

    fn bootstrap_target() -> ReconciliationTarget {
        ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap()
    }

    fn bootstrap_incarnation(target: &ReconciliationTarget) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            42,
            7420,
        )
        .unwrap()
    }

    fn test_receipt(sequence: u64, kind: LifecycleRecordKind) -> AuditDeliveryReceipt {
        AuditDeliveryReceipt::new(
            ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000001".into())
                .unwrap(),
            sequence,
            kind,
        )
    }

    #[test]
    fn production_transition_helpers_emit_only_completed_ordered_boundaries() {
        let progress = RecordingProgress::default();
        assert_eq!(
            retire_then_unload(&progress, || Ok(()), || Err("bootout failed".into())),
            Err("bootout failed".into())
        );
        assert_eq!(
            *progress.0.lock().unwrap(),
            [ReconciliationHistoryFact::RetirementAcknowledged]
        );

        let progress = RecordingProgress::default();
        assert_eq!(
            publish_definition(&progress, || Ok(()), || Err("sync failed".into())),
            Err("sync failed".into())
        );
        assert_eq!(
            *progress.0.lock().unwrap(),
            [ReconciliationHistoryFact::ServiceDefinitionPublished]
        );

        let progress = RecordingProgress::default();
        assert_eq!(
            run_bootstrap(&progress, || Err::<(), _>("spawn failed")),
            Err("spawn failed")
        );
        assert_eq!(
            *progress.0.lock().unwrap(),
            [ReconciliationHistoryFact::BootstrapCommandRequested]
        );

        let progress = RecordingProgress::default();
        run_bootstrap(&progress, || Ok::<_, &str>(())).unwrap();
        bootstrap_succeeded(&progress);
        assert_eq!(
            *progress.0.lock().unwrap(),
            [
                ReconciliationHistoryFact::BootstrapCommandRequested,
                ReconciliationHistoryFact::BootstrapCommandCompleted,
                ReconciliationHistoryFact::BootstrapCommandSucceeded,
            ]
        );
    }

    #[test]
    fn staging_cleanup_runs_only_when_the_journal_records_it_owed() {
        struct StagingProgress {
            events: Mutex<Vec<String>>,
            fail_primary_completion: bool,
            fail_primary_observation: bool,
        }

        impl GatewayReconciliationProgress for StagingProgress {
            fn readiness_invalidated(&self) {}

            fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
                Ok(())
            }

            fn history_observed(&self, _: ReconciliationHistoryFact) {}

            fn effect_planned(
                &self,
                plan_id: &str,
                _: &LifecyclePlanStep,
                cleanup: &[LifecyclePlanStep],
            ) -> Result<AuditDeliveryReceipt, GatewayError> {
                assert_eq!(plan_id, "stage-runtime");
                assert_eq!(cleanup.len(), 1);
                assert_eq!(cleanup[0].id(), "remove-published-runtime");
                // Owed only if staging did not succeed, so a success leaves
                // nothing pending for the next plan.
                assert_eq!(
                    cleanup[0].predicate(),
                    &LifecycleEffectPredicate::PrimaryNotAccepted
                );
                self.events.lock().unwrap().push("plan".into());
                Ok(test_receipt(1, LifecycleRecordKind::EffectPlan))
            }

            fn effect_completed(
                &self,
                _: &str,
                step_id: &str,
                _: &LifecycleCommandResult,
            ) -> Result<AuditDeliveryReceipt, GatewayError> {
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("complete:{step_id}"));
                if step_id == "primary" && self.fail_primary_completion {
                    Err(GatewayError::Registration(
                        "completion delivery failed".into(),
                    ))
                } else {
                    Ok(test_receipt(2, LifecycleRecordKind::EffectCompletion))
                }
            }

            fn physical_observed(
                &self,
                source: &LifecycleObservationSource,
                incarnation: Option<ReconciliationIncarnation>,
                target_artifact_present: bool,
            ) -> Result<LifecycleObservation, GatewayError> {
                let LifecycleObservationSource::Effect { step_id, .. } = source else {
                    panic!("staging observes its own steps");
                };
                self.events
                    .lock()
                    .unwrap()
                    .push(format!("observe:{step_id}"));
                if step_id == "primary" && self.fail_primary_observation {
                    return Err(GatewayError::Registration(
                        "observation delivery failed".into(),
                    ));
                }
                Ok(LifecycleObservation::new(
                    1,
                    incarnation,
                    target_artifact_present,
                ))
            }
        }

        let stage = |fail_primary_completion: bool,
                     fail_primary_observation: bool,
                     staged: Result<&'static str, String>| {
            let progress = StagingProgress {
                events: Mutex::new(Vec::new()),
                fail_primary_completion,
                fail_primary_observation,
            };
            let cleaned = AtomicBool::new(false);
            let result = run_planned_effect_with_cleanup(
                &progress,
                "stage-runtime",
                LifecycleEffect::StageRuntime {
                    fingerprint: "a".repeat(64),
                },
                super::staging_cleanup(&"a".repeat(64)).unwrap(),
                || staged,
                || {
                    cleaned.store(true, Ordering::SeqCst);
                    Ok(super::CleanupRun::Ran)
                },
                || Ok((None, false)),
            );
            (
                result,
                cleaned.load(Ordering::SeqCst),
                progress.events.into_inner().unwrap(),
            )
        };

        // Staging succeeded but its result was not recorded: nothing is owed,
        // nothing is undone, and the next registration prunes what is left.
        let (result, cleaned, events) = stage(true, false, Ok("published"));
        assert!(matches!(result, Err(super::RegisterFailure::Audit(_))));
        assert!(!cleaned);
        assert_eq!(events, ["plan", "complete:primary"]);

        // Staging failed, but its observation was not recorded: the journal
        // cannot yet accept the prune, so it does not run.
        let (result, cleaned, events) = stage(false, true, Err("copy failed".into()));
        assert!(matches!(result, Err(super::RegisterFailure::Audit(_))));
        assert!(!cleaned);
        assert_eq!(events, ["plan", "complete:primary", "observe:primary"]);

        // Staging failed and that was recorded: the prune is owed and runs.
        let (result, cleaned, events) = stage(false, false, Err("copy failed".into()));
        assert!(matches!(result, Err(super::RegisterFailure::Physical(_))));
        assert!(cleaned);
        assert_eq!(
            events,
            [
                "plan",
                "complete:primary",
                "observe:primary",
                "complete:remove-published-runtime",
                "observe:remove-published-runtime",
            ]
        );

        // Staging succeeded and was recorded: nothing is owed.
        let (result, cleaned, events) = stage(false, false, Ok("published"));
        assert!(result.is_ok());
        assert!(!cleaned);
        assert_eq!(events, ["plan", "complete:primary", "observe:primary"]);
    }

    #[test]
    fn bootstrap_cleanup_refuses_a_loaded_replacement() {
        let target = bootstrap_target();
        let replacement_target = ReconciliationTarget::new(
            target.service().to_owned(),
            "c".repeat(64),
            target.service_generation().to_owned(),
        )
        .unwrap();
        let unloaded = AtomicBool::new(false);
        let events = Arc::new(Mutex::new(Vec::new()));
        let progress = BootstrapCleanupProgress {
            events: events.clone(),
            completion_failure: false,
            observation_failure: false,
        };
        let result = cleanup_bootstrap_with(
            &progress,
            &bootstrap_cleanup_step(),
            &target,
            || Ok((true, Some(bootstrap_incarnation(&replacement_target)))),
            || {
                unloaded.store(true, Ordering::SeqCst);
                Ok(())
            },
            || Ok((true, Some(bootstrap_incarnation(&replacement_target)))),
        );

        let refusal =
            "the label is loaded, but not by the exact healthy bootstrap target; bootout refused";
        assert_eq!(result, Err(refusal.into()));
        assert!(!unloaded.load(Ordering::SeqCst));
        // The refusal is the cleanup's recorded result, with fresh state.
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.contains("unload-bootstrapped-service")));
        assert_eq!(
            bootstrap_cleanup_decision(false, None, &target),
            BootstrapCleanupDecision::AlreadyAbsent
        );
    }

    #[test]
    fn bootstrap_recovery_adopts_only_the_exact_target() {
        let target = bootstrap_target();
        let exact = bootstrap_incarnation(&target);
        let replacement_target = ReconciliationTarget::new(
            target.service().to_owned(),
            "c".repeat(64),
            target.service_generation().to_owned(),
        )
        .unwrap();
        let replacement = bootstrap_incarnation(&replacement_target);

        assert_eq!(
            bootstrap_recovery_decision(Some(&exact), &target),
            BootstrapRecoveryDecision::Adopt(exact)
        );
        assert_eq!(
            bootstrap_recovery_decision(None, &target),
            BootstrapRecoveryDecision::Refuse
        );
        assert_eq!(
            bootstrap_recovery_decision(Some(&replacement), &target),
            BootstrapRecoveryDecision::Refuse
        );
    }

    #[test]
    fn recovery_probes_the_latest_exact_target_port_before_stale_prior_state() {
        let target = bootstrap_target();
        let before = bootstrap_incarnation(&target);
        let latest_incarnation = ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440001".into(),
            43,
            7431,
        )
        .unwrap();
        let latest = LifecycleObservation::new(2, Some(latest_incarnation), true);
        let definition = json!({
            "EnvironmentVariables": {
                "NESSA_RUNTIME_FINGERPRINT": target.runtime_fingerprint(),
                "NESSA_SERVICE_GENERATION": target.service_generation(),
                "NESSA_PORT": "7442"
            }
        });

        assert_eq!(
            recovery_probe_port(
                &target,
                Some(&latest),
                Some(&definition),
                Some(&before),
                7453
            ),
            7431
        );
    }

    #[test]
    fn recovery_uses_only_an_installed_definition_for_the_exact_target() {
        let target = bootstrap_target();
        let exact = json!({
            "EnvironmentVariables": {
                "NESSA_RUNTIME_FINGERPRINT": target.runtime_fingerprint(),
                "NESSA_SERVICE_GENERATION": target.service_generation(),
                "NESSA_PORT": "7442"
            }
        });
        assert_eq!(
            recovery_probe_port(&target, None, Some(&exact), None, 7453),
            7442
        );

        for definition in [
            json!({"EnvironmentVariables": {
                "NESSA_RUNTIME_FINGERPRINT": "c".repeat(64),
                "NESSA_SERVICE_GENERATION": target.service_generation(),
                "NESSA_PORT": "7442"
            }}),
            json!({"EnvironmentVariables": {
                "NESSA_RUNTIME_FINGERPRINT": target.runtime_fingerprint(),
                "NESSA_SERVICE_GENERATION": "d".repeat(64),
                "NESSA_PORT": "7442"
            }}),
            json!({"EnvironmentVariables": {
                "NESSA_RUNTIME_FINGERPRINT": target.runtime_fingerprint(),
                "NESSA_SERVICE_GENERATION": target.service_generation(),
                "NESSA_PORT": "not-a-port"
            }}),
        ] {
            assert_eq!(
                recovery_probe_port(&target, None, Some(&definition), None, 7453),
                7453
            );
        }
    }

    #[test]
    fn prune_observation_distinguishes_absence_replacement_and_unknown_state() {
        let root = tempfile::tempdir().unwrap();
        let artifact = root.path().join("artifact");
        assert_eq!(artifact_presence(&artifact), Ok(false));

        fs::write(&artifact, b"replacement").unwrap();
        assert_eq!(artifact_presence(&artifact), Ok(true));

        let unreadable_name = "x".repeat(300);
        assert!(artifact_presence(&root.path().join(unreadable_name)).is_err());
    }

    #[test]
    fn bootstrap_cleanup_preserves_physical_result_when_audit_remains_unavailable() {
        for (completion_failure, observation_failure, expected) in [
            (true, false, "cleanup completion unavailable"),
            (false, true, "cleanup observation unavailable"),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let progress = BootstrapCleanupProgress {
                events: events.clone(),
                completion_failure,
                observation_failure,
            };
            let target = bootstrap_target();
            let result = cleanup_bootstrap_with(
                &progress,
                &bootstrap_cleanup_step(),
                &target,
                || Ok((true, Some(bootstrap_incarnation(&target)))),
                {
                    let events = events.clone();
                    move || {
                        events.lock().unwrap().push("unload".into());
                        Ok(())
                    }
                },
                || Ok((false, None)),
            );

            let error = result.unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(
                error.contains("cleanup command result was Accepted"),
                "{error}"
            );
            assert_eq!(
                events.lock().unwrap().first().map(String::as_str),
                Some("unload")
            );
        }
    }

    #[test]
    fn bootstrap_cleanup_records_a_failed_unload_and_fresh_state() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let progress = BootstrapCleanupProgress {
            events: events.clone(),
            completion_failure: false,
            observation_failure: false,
        };
        let target = bootstrap_target();
        let incarnation = bootstrap_incarnation(&target);
        let incarnation_after = incarnation.clone();
        let result = cleanup_bootstrap_with(
            &progress,
            &bootstrap_cleanup_step(),
            &target,
            || Ok((true, Some(incarnation.clone()))),
            || Err("bootout failed".into()),
            || Ok((true, Some(incarnation_after))),
        );

        assert_eq!(result, Err("bootout failed".into()));
        assert_eq!(
            *events.lock().unwrap(),
            ["complete:unload-bootstrapped-service", "observe-cleanup",]
        );
    }

    fn temporary_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nessa-gateway-{name}-{}", std::process::id()))
    }

    #[test]
    fn stop_authority_requires_the_same_live_runtime_incarnation() {
        let gateway = ReconciledGateway::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            "b".repeat(64),
            42,
            7420,
        );
        let status = ServiceStatus {
            loaded: true,
            pid: Some(42),
            process_identity_known: true,
            last_exit: LastExit::NeverExited,
        };
        let current = Health::Managed(ManagedRuntime {
            fingerprint: "a".repeat(64),
            generation: "b".repeat(64),
            instance: "550e8400-e29b-41d4-a716-446655440000".into(),
            pid: 42,
        });
        assert!(matches_reconciled_gateway(
            &gateway,
            &status,
            Some(&current)
        ));

        for replacement in [
            ManagedRuntime {
                fingerprint: "c".repeat(64),
                generation: "b".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "d".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "b".repeat(64),
                instance: "660e8400-e29b-41d4-a716-446655440000".into(),
                pid: 42,
            },
            ManagedRuntime {
                fingerprint: "a".repeat(64),
                generation: "b".repeat(64),
                instance: "550e8400-e29b-41d4-a716-446655440000".into(),
                pid: 43,
            },
        ] {
            assert!(!matches_reconciled_gateway(
                &gateway,
                &status,
                Some(&Health::Managed(replacement))
            ));
        }
        assert!(!matches_reconciled_gateway(
            &gateway,
            &ServiceStatus {
                loaded: true,
                pid: Some(43),
                process_identity_known: true,
                last_exit: LastExit::NeverExited,
            },
            Some(&current)
        ));
        assert!(!matches_reconciled_gateway(
            &gateway,
            &ServiceStatus {
                loaded: true,
                pid: Some(42),
                process_identity_known: false,
                last_exit: LastExit::NeverExited,
            },
            Some(&current)
        ));
        assert!(!matches_reconciled_gateway(
            &gateway,
            &status,
            Some(&Health::Legacy)
        ));
    }

    #[test]
    fn non_production_data_rejects_a_symlinked_stage_without_touching_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = temporary_directory("symlink-root");
        let outside = temporary_directory("symlink-target");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let base = root.join(".nessa");
        nessa_local_storage::create_directory(&base).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&outside, base.join("dev")).unwrap();

        assert!(prepare_data_directory(&base, "dev", Some("worktree")).is_err());
        assert!(!outside.join("instances/worktree").exists());
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn runtime_manifest_requires_a_sha256_fingerprint() {
        let directory = temporary_directory("manifest");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("manifest.json"),
            r#"{"fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert_eq!(runtime_fingerprint(&directory).unwrap(), "a".repeat(64));
        fs::write(
            directory.join("manifest.json"),
            r#"{"fingerprint":"not-a-digest"}"#,
        )
        .unwrap();
        assert_eq!(
            runtime_fingerprint(&directory),
            Err("Invalid runtime fingerprint".into())
        );
        fs::write(
            directory.join("manifest.json"),
            format!(r#"{{"fingerprint":"{}"}}"#, "A".repeat(64)),
        )
        .unwrap();
        assert_eq!(
            runtime_fingerprint(&directory),
            Err("Invalid runtime fingerprint".into())
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unreadable_process_identity_preserves_the_service_and_names_safe_recovery() {
        let message = unreadable_process_identity();
        assert!(message.contains("preserved rather than risking a live gateway"));
        assert!(message.contains("Quit Nessa and open it again"));
        assert!(message.contains("lifecycle evidence"));
    }

    /// A service that gave up is loaded, has no process, and launchd will
    /// never start it again — so this host is the only thing that can, and the
    /// cause may well have been repaired since. It may boot out only what it
    /// can establish has no process of its own and wrote the record itself.
    #[test]
    fn a_service_that_gave_up_may_be_replaced_only_on_its_own_unambiguous_absence() {
        assert!(gave_up_retry(None, true, true));
        // No record, or one belonging to another registration.
        assert!(!gave_up_retry(None, true, false));
        // An answer we did not get is not absence.
        assert!(!gave_up_retry(None, false, true));
        // Something is running under this label; a record is not licence to
        // boot it out.
        assert!(!gave_up_retry(Some(42), true, true));
        assert!(!gave_up_retry(Some(42), false, true));
    }

    /// The generation is read from the definition that is actually installed,
    /// which is what makes a record this registration's own rather than
    /// whatever ran in this directory before it.
    #[test]
    fn the_installed_generation_comes_from_the_installed_definition() {
        let generation = "a".repeat(64);
        let installed = json!({"EnvironmentVariables": {"NESSA_SERVICE_GENERATION": generation}});
        assert_eq!(installed_generation(Some(&installed)), Some(&*generation));
        assert_eq!(installed_generation(None), None);
        for incomplete in [
            json!({}),
            json!({"EnvironmentVariables": {}}),
            json!({"EnvironmentVariables": {"NESSA_SERVICE_GENERATION": 7}}),
        ] {
            assert_eq!(installed_generation(Some(&incomplete)), None);
        }
    }

    /// The generic sentence said nothing about why. A gateway that recorded a
    /// reason is quoted instead, and a service that recorded nothing still
    /// gets the only honest answer there is.
    #[test]
    fn an_unavailable_service_says_why_when_the_gateway_said_why() {
        let recorded = startup::parse_record(
            json!({
                "reason": "credentialRegistryInvalid",
                "exitCode": 28,
                "message": "authentication setup failed: credential registry is invalid",
                "serviceGeneration": "a".repeat(64),
                "processId": 4711,
            })
            .to_string()
            .as_bytes(),
        )
        .expect("record");
        let said = unavailable_service(Some(&recorded), 7420);
        assert!(
            said.contains("credential registry is not one this version of Nessa can read"),
            "{said}"
        );
        assert!(said.contains("preserved"), "{said}");
        assert_eq!(
            unavailable_service(None, 7420),
            "Loaded gateway has no valid health response; service was preserved"
        );
    }

    #[test]
    fn bootstrap_failure_reports_what_launchd_shows() {
        assert_eq!(finish_bootstrap(Ok(()), || Ok(false), || Ok(false)), Ok(()));

        let error = finish_bootstrap(
            Err(BootstrapFailure::CouldNotRun("bootstrap failed".into())),
            || Ok(false),
            || Ok(false),
        )
        .unwrap_err();
        assert!(error.contains("launchd reports no loaded service"));

        let error = finish_bootstrap(
            Err(BootstrapFailure::CouldNotRun("bootstrap failed".into())),
            || Ok(true),
            || Ok(false),
        )
        .unwrap_err();
        assert!(error.contains("launchd reports a loaded service"));

        let error = finish_bootstrap(
            Err(BootstrapFailure::CouldNotRun("bootstrap failed".into())),
            || Err("ambiguous status".into()),
            || Ok(false),
        )
        .unwrap_err();
        assert!(error.contains("could not be verified"));
        assert!(error.contains("ambiguous status"));
    }

    #[test]
    fn disabled_login_item_needs_exit_five_unloaded_and_disabled_to_name_settings() {
        let refusal = BootstrapFailure::Refused {
            status: Some(5),
            detail: "Bootstrap failed: 5: Input/output error".into(),
        };
        let blocked = finish_bootstrap(Err(refusal), || Ok(false), || Ok(true)).unwrap_err();
        assert!(blocked.contains("System Settings → General → Login Items"));

        for (status, loaded, disabled) in [
            (Some(4), false, true),
            (Some(5), true, true),
            (Some(5), false, false),
        ] {
            let error = finish_bootstrap(
                Err(BootstrapFailure::Refused {
                    status,
                    detail: "bootstrap refused".into(),
                }),
                || Ok(loaded),
                || Ok(disabled),
            )
            .unwrap_err();
            assert!(!error.contains("System Settings"), "{error}");
        }

        let diagnosis_failed = finish_bootstrap(
            Err(BootstrapFailure::Refused {
                status: Some(5),
                detail: "original bootstrap refusal".into(),
            }),
            || Ok(false),
            || Err("print-disabled exceeded its deadline".into()),
        )
        .unwrap_err();
        assert!(diagnosis_failed.contains("original bootstrap refusal"));
        assert!(diagnosis_failed.contains("print-disabled exceeded its deadline"));
        assert!(!diagnosis_failed.contains("System Settings"));
    }

    #[test]
    fn disabled_service_parser_requires_the_exact_label_and_disabled_value() {
        let output = r#"disabled services = {
            "so.nessa.gateway.prod" => disabled
            "so.nessa.gateway.other" => enabled
        }"#;
        assert!(disabled_service(output, "so.nessa.gateway.prod").unwrap());
        assert!(!disabled_service(output, "so.nessa.gateway.other").unwrap());
        assert!(!disabled_service(output, "so.nessa.gateway").unwrap());
        assert!(disabled_service("not launchctl output", "so.nessa.gateway.prod").is_err());
        assert!(disabled_service(
            "disabled services = {\n\"so.nessa.gateway.prod\" => mystery\n}",
            "so.nessa.gateway.prod"
        )
        .is_err());
    }

    #[test]
    fn registered_service_must_match_runtime_path_and_content() {
        let directory = temporary_directory("plist");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let plist = directory.join("gateway.plist");
        fs::write(
            &plist,
            r#"{"ProgramArguments":["/Applications/Nessa.app/runtime/nessa","server","--desktop-runtime","/Applications/Nessa.app/runtime"],"WorkingDirectory":"/Users/me/.nessa","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":"current","HOME":"/Users/me","NESSA_STAGE":"prod","NESSA_HOST":"127.0.0.1","NESSA_PORT":"7420","PATH":"/usr/bin:/bin:/usr/sbin:/sbin","NESSA_AGENT_PATH":"/opt/homebrew/bin:/usr/bin:/bin"}}"#,
        )
        .unwrap();
        let expected: Value = serde_json::from_slice(&fs::read(&plist).unwrap()).unwrap();
        assert!(service_matches(&plist, &expected));
        for (key, replacement) in [
            ("KeepAlive", json!(false)),
            ("WorkingDirectory", json!("/Users/me/other")),
            (
                "ProgramArguments",
                json!(["/Applications/Other.app/runtime/nessa"]),
            ),
        ] {
            let mut changed = expected.clone();
            changed[key] = replacement;
            assert!(!service_matches(&plist, &changed));
        }
        let mut changed = expected.clone();
        changed["EnvironmentVariables"]["CLAUDE_CONFIG_DIR"] = json!("/new/provider");
        assert!(!service_matches(&plist, &changed));
        // The agent's path is part of the service definition, so changing it is
        // a re-registration and not something that quietly takes effect.
        let mut changed = expected.clone();
        changed["EnvironmentVariables"]["NESSA_AGENT_PATH"] = json!("/opt/homebrew/bin:/usr/bin");
        assert!(!service_matches(&plist, &changed));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn service_environment_uses_only_durable_inputs_and_names_provider_changes() {
        let base = ServiceConfiguration::new(
            "prod".into(),
            PathBuf::from("/Users/me/.nessa"),
            Some("work".into()),
            7420,
            None,
        )
        .unwrap();
        let path = SearchPath::parse("/opt/homebrew/bin:/usr/bin:/bin").unwrap();
        let first = service_environment(&base, Path::new("/Users/me"), &path, "fingerprint");
        let same = service_environment(&base, Path::new("/Users/me"), &path, "fingerprint");
        assert_eq!(first, same);
        for inherited in ["USER", "LOGNAME", "TMPDIR"] {
            assert!(!first.contains_key(inherited));
        }

        let changed = ServiceConfiguration::new(
            "prod".into(),
            PathBuf::from("/Users/me/.nessa"),
            Some("work".into()),
            7420,
            Some(PathBuf::from("/Users/me/.claude-work")),
        )
        .unwrap();
        let changed = service_environment(&changed, Path::new("/Users/me"), &path, "fingerprint");
        let differences = changed
            .iter()
            .filter(|(key, value)| first.get(*key) != Some(*value))
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(differences, ["CLAUDE_CONFIG_DIR"]);
    }

    /// The two ends of one variable, which no type connects: this host writes
    /// it into the service definition and the gateway reads it out of its own
    /// environment. Renaming it on one side alone leaves an agent quietly back
    /// on the system path, which is the failure this whole change is about.
    #[test]
    fn the_gateway_reads_the_agent_path_variable_this_host_writes() {
        let gateway = include_str!("../../../../crates/nessa-server/src/composition/agent.rs");
        assert!(
            include_str!("macos.rs").contains(r#"("NESSA_AGENT_PATH", agent_path"#),
            "this host no longer registers NESSA_AGENT_PATH"
        );
        assert!(
            gateway.contains(r#"var_os("NESSA_AGENT_PATH")"#),
            "crates/nessa-server/src/composition/agent.rs does not read NESSA_AGENT_PATH"
        );
    }

    /// The whole point of the retained path: launch context does not rewrite a
    /// healthy definition for the exact same runtime.
    #[test]
    fn shell_changes_keep_the_registered_path_for_the_same_runtime() {
        let runtime = Path::new("/Users/me/Library/Application Support/Nessa/runtimes/abc");
        let installed = json!({
            "ProgramArguments": launch_settings(runtime),
            "EnvironmentVariables": {"NESSA_AGENT_PATH": "/opt/homebrew/bin:/usr/bin:/bin"}
        });
        let registered = SearchPath::parse("/opt/homebrew/bin:/usr/bin:/bin").unwrap();
        let changed = SearchPath::parse("/new/shell/bin:/usr/bin:/bin").unwrap();
        assert_eq!(
            registered_agent_path(Some(&changed), Some(&installed), runtime),
            registered
        );
        assert_eq!(
            registered_agent_path(None, Some(&installed), runtime),
            registered
        );
    }

    #[test]
    fn a_new_runtime_uses_the_current_resolved_path() {
        let old_runtime = Path::new("/staged/old");
        let new_runtime = Path::new("/staged/new");
        let installed = json!({
            "ProgramArguments": launch_settings(old_runtime),
            "EnvironmentVariables": {"NESSA_AGENT_PATH": "/old/bin:/usr/bin"}
        });
        let resolved = SearchPath::parse("/new/bin:/usr/bin").unwrap();

        assert_eq!(
            registered_agent_path(Some(&resolved), Some(&installed), new_runtime),
            resolved
        );
    }

    /// A first launch with no login shell to read, and a definition that has
    /// nothing usable to keep, still leaves the agent a working path.
    #[test]
    fn nothing_to_resolve_and_nothing_registered_falls_back_to_the_system_path() {
        let runtime = Path::new("/staged/runtime");
        for installed in [
            None,
            Some(json!({})),
            Some(json!({"EnvironmentVariables": {}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": ""}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": "relative:./bin"}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": 7}})),
            Some(json!({"EnvironmentVariables": {"NESSA_AGENT_PATH": "/staged/runtime"}})),
        ] {
            assert_eq!(
                registered_agent_path(None, installed.as_ref(), runtime),
                SearchPath::system(),
                "{installed:?}"
            );
        }
    }

    /// Nessa's own `node`, `nessa` and `nessa-mcp` live in the staged runtime.
    /// Whichever source the path came from, that directory is not on it.
    #[test]
    fn the_staged_runtime_never_reaches_the_agents_path() {
        let runtime = Path::new("/staged/runtime");
        let resolved = SearchPath::parse("/staged/runtime:/opt/homebrew/bin:/usr/bin").unwrap();
        assert_eq!(
            registered_agent_path(Some(&resolved), None, runtime).as_str(),
            "/opt/homebrew/bin:/usr/bin"
        );
        let installed = json!({
            "ProgramArguments": launch_settings(runtime),
            "EnvironmentVariables": {"NESSA_AGENT_PATH": "/usr/bin:/staged/runtime"}
        });
        assert_eq!(
            registered_agent_path(None, Some(&installed), runtime).as_str(),
            "/usr/bin"
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    //! Recovery against a journal that applies the domain's own lifecycle
    //! rules, so an outcome the domain would refuse fails the test, and every
    //! row asserts what recovery wrote.
    use super::*;
    use crate::gateway::{
        application::{
            GatewayReconciliationOutcome, GatewayReconciliationOutcomeError,
            GatewayReconciliationRequest,
        },
        domain::value_objects::{
            LifecycleHistory, LifecycleRecord, LifecycleRecordPayload, ReconciliationCorrelation,
            ReconciliationEvidence, ReconciliationInitiator, ServiceManager,
        },
    };
    use std::sync::Mutex;

    const UNRETURNED: &str = "The step's result was not recorded before recovery";
    const NOT_RUN: &str =
        "Not run by recovery; whether the interrupted attempt ran it is not recorded";

    fn correlation(serial: u64) -> ReconciliationCorrelation {
        ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{serial:012x}")).unwrap()
    }

    fn indeterminate(reason: &str) -> LifecycleCommandResult {
        LifecycleCommandResult::Indeterminate(reason.into())
    }

    struct Scenario {
        home: tempfile::TempDir,
        label: String,
        target: ReconciliationTarget,
        records: Vec<LifecycleRecordPayload>,
        before: Option<ReconciliationIncarnation>,
        /// What launchd and the health endpoint report.
        running: Option<u32>,
        loaded: bool,
        /// The runtime the running gateway reports, when not the target's.
        running_fingerprint: Option<String>,
    }

    /// launchd and health as a test sets them. A command is allowed only
    /// where the test scripts one, so recovery running any command panics.
    struct FakeLaunchctl {
        target: ReconciliationTarget,
        state: Mutex<FakeState>,
        /// The exit code `bootstrap` returns, and the state it leaves.
        bootstrap: Option<(i32, FakeState)>,
        /// Whether commands may run at all; recovery's fake allows none.
        commands: bool,
        /// What a bootout returns when it fails.
        bootout_failure: Option<String>,
        /// What a signal returns.
        signal_result: Option<LifecycleCommandResult>,
        bootouts: Mutex<usize>,
        signals: Mutex<Vec<String>>,
    }

    #[derive(Clone, Default)]
    struct FakeState {
        loaded: bool,
        running: Option<u32>,
        running_fingerprint: Option<String>,
    }

    impl FakeLaunchctl {
        fn new(target: &ReconciliationTarget, state: FakeState) -> Self {
            Self {
                target: target.clone(),
                state: Mutex::new(state),
                bootstrap: None,
                commands: false,
                bootout_failure: None,
                signal_result: None,
                bootouts: Mutex::new(0),
                signals: Mutex::new(Vec::new()),
            }
        }
    }

    impl Launchctl for FakeLaunchctl {
        fn status(&self, _: &str) -> Result<ServiceStatus, String> {
            let state = self.state.lock().unwrap();
            Ok(ServiceStatus {
                loaded: state.loaded,
                pid: state.running,
                process_identity_known: true,
                last_exit: startup::LastExit::Unknown,
            })
        }
        fn health(&self, _: u16) -> Option<Health> {
            let state = self.state.lock().unwrap();
            state.running.map(|pid| {
                Health::Managed(ManagedRuntime {
                    fingerprint: state
                        .running_fingerprint
                        .clone()
                        .unwrap_or_else(|| self.target.runtime_fingerprint().into()),
                    generation: self.target.service_generation().into(),
                    instance: "550e8400-e29b-41d4-a716-446655440001".into(),
                    pid,
                })
            })
        }
        fn bootstrap(&self, _: &str, _: &Path) -> std::io::Result<Output> {
            use std::os::unix::process::ExitStatusExt;
            assert!(self.commands, "recovery runs no command");
            let (code, after) = self.bootstrap.clone().expect("no bootstrap was scripted");
            *self.state.lock().unwrap() = after;
            Ok(Output {
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: Vec::new(),
                stderr: b"Bootstrap failed: 5: Input/output error".to_vec(),
            })
        }
        fn bootout(&self, _: &str) -> Result<(), String> {
            assert!(self.commands, "recovery runs no command");
            *self.bootouts.lock().unwrap() += 1;
            if let Some(failure) = &self.bootout_failure {
                return Err(failure.clone());
            }
            *self.state.lock().unwrap() = FakeState::default();
            Ok(())
        }
        fn signal(
            &self,
            _: &str,
            signal: &str,
            _: &mut dyn FnMut() -> bool,
        ) -> LifecycleCommandResult {
            assert!(self.commands, "recovery runs no command");
            self.signals.lock().unwrap().push(signal.into());
            self.signal_result
                .clone()
                .expect("no signal result was scripted")
        }
    }

    impl Scenario {
        fn new(before: Option<ReconciliationIncarnation>) -> Self {
            let uid = unsafe { libc::getuid() };
            let label = format!("so.nessa.gateway.recovery-test-{}", std::process::id());
            let target = ReconciliationTarget::new(
                format!("gui/{uid}/{label}"),
                "a".repeat(64),
                "b".repeat(64),
            )
            .unwrap();
            Self {
                home: tempfile::tempdir().unwrap(),
                label,
                records: vec![LifecycleRecordPayload::Intent {
                    request_correlation: correlation(1),
                    cause: ReconciliationCause::Startup,
                    initiator: ReconciliationInitiator::DesktopHost,
                    target: target.clone(),
                    before: before.clone(),
                }],
                target,
                before,
                running: None,
                loaded: false,
                running_fingerprint: None,
            }
        }

        /// launchd runs another runtime under the label.
        fn other_running(mut self, pid: u32) -> Self {
            self.loaded = true;
            self.running = Some(pid);
            self.running_fingerprint = Some("c".repeat(64));
            self
        }

        /// launchd runs the exact target with `pid`.
        fn target_running(mut self, pid: u32) -> Self {
            self.loaded = true;
            self.running = Some(pid);
            self
        }

        fn label_loaded(mut self) -> Self {
            self.loaded = true;
            self
        }

        fn plan(
            mut self,
            primary: LifecycleEffect,
            cleanup: Option<(LifecycleEffect, LifecycleEffectPredicate)>,
        ) -> Self {
            self.records.push(LifecycleRecordPayload::EffectPlan {
                plan_id: "plan".into(),
                expected_before: self.before.clone(),
                target: self.target.clone(),
                primary: LifecyclePlanStep::new(
                    "primary".into(),
                    primary,
                    LifecycleEffectPredicate::Always,
                )
                .unwrap(),
                cleanup: cleanup
                    .map(|(effect, predicate)| {
                        LifecyclePlanStep::new("cleanup".into(), effect, predicate).unwrap()
                    })
                    .into_iter()
                    .collect(),
            });
            self
        }

        fn completed(mut self, step: &str, result: LifecycleCommandResult) -> Self {
            self.records.push(LifecycleRecordPayload::EffectCompletion {
                plan_id: "plan".into(),
                step_id: step.into(),
                result,
            });
            self
        }

        fn observed(mut self, step: &str, version: u64, artifact: bool) -> Self {
            self.records.push(LifecycleRecordPayload::Observation {
                source: LifecycleObservationSource::Effect {
                    plan_id: "plan".into(),
                    step_id: step.into(),
                },
                state: LifecycleObservation::new(version, None, artifact),
            });
            self
        }

        fn runtimes(&self) -> PathBuf {
            self.home
                .path()
                .join("Library/Application Support/Nessa/gateway-runtimes")
                .join(&self.label)
        }

        fn published(self) -> Self {
            let agents = self.home.path().join("Library/LaunchAgents");
            fs::create_dir_all(&agents).unwrap();
            let definition = serde_json::json!({
                "Label": self.label,
                "EnvironmentVariables": {"NESSA_SERVICE_GENERATION": self.target.service_generation()},
            });
            fs::write(
                agents.join(format!("{}.plist", self.label)),
                convert_definition_to_xml(&serde_json::to_vec(&definition).unwrap()).unwrap(),
            )
            .unwrap();
            self
        }

        fn stage(&self) -> LifecycleEffect {
            LifecycleEffect::StageRuntime {
                fingerprint: self.target.runtime_fingerprint().into(),
            }
        }

        fn prune(&self) -> LifecycleEffect {
            LifecycleEffect::PruneRuntime {
                fingerprint: self.target.runtime_fingerprint().into(),
            }
        }

        fn recover(&self) -> Recovered {
            let namespace = self.target.service().to_owned();
            let records = self
                .records
                .iter()
                .enumerate()
                .map(|(sequence, payload)| {
                    LifecycleRecord::new(
                        namespace.clone(),
                        correlation(2),
                        sequence as u64,
                        payload.clone(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let history = LifecycleHistory::restore(ServiceManager::Launchd, &records).unwrap();
            let request = GatewayReconciliationRequest::new(
                correlation(1),
                ReconciliationEvidence::new(
                    ReconciliationCause::Startup,
                    ReconciliationInitiator::DesktopHost,
                )
                .unwrap(),
            );
            let recovery = GatewayLifecycleRecovery::from_history(
                GatewayReconciliationAttempt::new(correlation(2), request).unwrap(),
                self.target.clone(),
                self.before.clone(),
                &history,
            );
            let journal = DomainJournal {
                namespace,
                history: Mutex::new(history),
                written: Mutex::new(Vec::new()),
            };
            let configuration = ServiceConfiguration::new(
                "recoverytest".into(),
                self.home.path().join("nessa"),
                None,
                7398,
                None,
            )
            .unwrap();
            let host = Launchd::new(
                Arc::new(LaunchctlDisabledServiceStatus),
                Arc::new(FakeLaunchctl::new(
                    &self.target,
                    FakeState {
                        loaded: self.loaded,
                        running: self.running,
                        running_fingerprint: self.running_fingerprint.clone(),
                    },
                )),
                configuration,
                self.home.path().to_path_buf(),
            );
            let result = host.recover(&recovery, &journal);
            Recovered {
                result,
                terminal: journal.history.into_inner().unwrap().is_terminal(),
                written: journal.written.into_inner().unwrap(),
            }
        }
    }

    struct Recovered {
        result: Result<(), GatewayError>,
        terminal: bool,
        written: Vec<LifecycleRecordPayload>,
    }

    impl Recovered {
        /// The completions and observations recovery wrote, then its outcome's
        /// phase and message.
        fn closed(self, row: &str) -> (Vec<Written>, LifecycleFailedPhase, String) {
            if let Err(error) = &self.result {
                panic!("{row}: {error}");
            }
            assert!(self.terminal, "{row}");
            let mut written = Vec::new();
            let mut outcome = None;
            for payload in self.written {
                match payload {
                    LifecycleRecordPayload::EffectCompletion {
                        step_id, result, ..
                    } => written.push(Written::Completed(step_id, result)),
                    LifecycleRecordPayload::Observation { source, state } => {
                        written.push(Written::Observed(source, state.target_artifact_present()))
                    }
                    LifecycleRecordPayload::Outcome {
                        physical: LifecyclePhysicalOutcome::Failed { phase, message },
                        cleanup: ReconciliationCleanupDecision::RetainPrior,
                        ..
                    } => outcome = Some((phase, message)),
                    other => panic!("{row}: unexpected record {other:?}"),
                }
            }
            let (phase, message) = outcome.unwrap_or_else(|| panic!("{row}: no outcome"));
            (written, phase, message)
        }
    }

    impl Recovered {
        /// The records recovery wrote before a Confirmed adoption.
        fn adopted(self, row: &str) -> Vec<Written> {
            if let Err(error) = &self.result {
                panic!("{row}: {error}");
            }
            assert!(self.terminal, "{row}");
            let mut written = Vec::new();
            let mut confirmed = false;
            for payload in self.written {
                match payload {
                    LifecycleRecordPayload::EffectCompletion {
                        step_id, result, ..
                    } => written.push(Written::Completed(step_id, result)),
                    LifecycleRecordPayload::Observation { source, state } => {
                        written.push(Written::Observed(source, state.target_artifact_present()))
                    }
                    LifecycleRecordPayload::Outcome {
                        physical: LifecyclePhysicalOutcome::Confirmed(_),
                        cleanup: ReconciliationCleanupDecision::AdoptClaimed,
                        ..
                    } => confirmed = true,
                    other => panic!("{row}: unexpected record {other:?}"),
                }
            }
            assert!(confirmed, "{row}: no adoption");
            written
        }
    }

    #[derive(Debug, PartialEq)]
    enum Written {
        Completed(String, LifecycleCommandResult),
        Observed(LifecycleObservationSource, bool),
    }

    fn observed(step: &str, artifact: bool) -> Written {
        Written::Observed(
            LifecycleObservationSource::Effect {
                plan_id: "plan".into(),
                step_id: step.into(),
            },
            artifact,
        )
    }

    fn completed(step: &str, reason: &str) -> Written {
        Written::Completed(step.into(), indeterminate(reason))
    }

    struct DomainJournal {
        namespace: String,
        history: Mutex<LifecycleHistory>,
        written: Mutex<Vec<LifecycleRecordPayload>>,
    }

    impl DomainJournal {
        fn append(
            &self,
            payload: LifecycleRecordPayload,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            let mut history = self.history.lock().unwrap();
            let sequence = history.next_sequence();
            let kind = payload.kind();
            let record = LifecycleRecord::new(
                self.namespace.clone(),
                correlation(2),
                sequence,
                payload.clone(),
            )
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
            history
                .append(&record)
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            self.written.lock().unwrap().push(payload);
            Ok(AuditDeliveryReceipt::new(correlation(2), sequence, kind))
        }
    }

    impl GatewayReconciliationJournalSession for DomainJournal {
        fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
            unreachable!("recovery admits no intent")
        }
        fn outcome(
            &self,
            _: &GatewayReconciliationOutcome,
        ) -> Result<(), GatewayReconciliationOutcomeError> {
            unreachable!("recovery records a physical outcome")
        }
        fn joined(&self, _: &GatewayReconciliationRequest) -> Result<(), GatewayError> {
            unreachable!("recovery joins no request")
        }
        fn effect_plan(
            &self,
            _: &str,
            _: Option<&ReconciliationIncarnation>,
            _: &ReconciliationTarget,
            _: &LifecyclePlanStep,
            _: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            unreachable!("recovery plans no effect")
        }
        fn effect_completion(
            &self,
            plan_id: &str,
            step_id: &str,
            result: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.append(LifecycleRecordPayload::EffectCompletion {
                plan_id: plan_id.into(),
                step_id: step_id.into(),
                result: result.clone(),
            })
        }
        fn observation(
            &self,
            source: &LifecycleObservationSource,
            state: &LifecycleObservation,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.append(LifecycleRecordPayload::Observation {
                source: source.clone(),
                state: state.clone(),
            })
        }
        fn physical_outcome(
            &self,
            physical: &LifecyclePhysicalOutcome,
            last_confirmed: Option<&LifecycleObservation>,
            cleanup: ReconciliationCleanupDecision,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.append(LifecycleRecordPayload::Outcome {
                physical: physical.clone(),
                last_confirmed: last_confirmed.cloned(),
                cleanup,
            })
        }
    }

    #[test]
    fn settled_plans_close_on_their_last_saved_observation() {
        // #210: a first staging succeeded and was observed with its runtime
        // present; its cleanup is owed only if staging failed.
        let scenario = Scenario::new(None);
        let (stage, prune) = (scenario.stage(), scenario.prune());
        let (written, phase, message) = scenario
            .plan(
                stage,
                Some((prune, LifecycleEffectPredicate::PrimaryNotAccepted)),
            )
            .completed("primary", LifecycleCommandResult::Accepted)
            .observed("primary", 1, true)
            .recover()
            .closed("settled staging");
        assert!(written.is_empty());
        assert_eq!(phase, LifecycleFailedPhase::Observation);
        assert!(message.contains("last saved observation"), "{message}");

        let scenario = Scenario::new(None).published();
        let publish = LifecycleEffect::PublishServiceDefinition {
            target: scenario.target.clone(),
        };
        let (written, _, _) = scenario
            .plan(publish, None)
            .completed("primary", LifecycleCommandResult::Accepted)
            .observed("primary", 1, true)
            .recover()
            .closed("settled publication");
        assert!(written.is_empty());
    }

    #[test]
    fn an_intent_without_a_plan_closes_on_the_fresh_observation() {
        let prior_target = ReconciliationTarget::new(
            Scenario::new(None).target.service().into(),
            "c".repeat(64),
            "d".repeat(64),
        )
        .unwrap();
        // The prior gateway is gone, or restarted with another PID.
        let prior = ReconciliationIncarnation::new(
            prior_target,
            "550e8400-e29b-41d4-a716-446655440000".into(),
            4242,
            7398,
        )
        .unwrap();
        let (written, phase, _) = Scenario::new(Some(prior)).recover().closed("no plan");
        assert_eq!(
            written,
            [Written::Observed(LifecycleObservationSource::Intent, false)]
        );
        assert_eq!(phase, LifecycleFailedPhase::Observation);
        // An installed plist of the target is recorded present.
        let (written, _, _) = Scenario::new(None)
            .published()
            .recover()
            .closed("no plan, target installed");
        assert_eq!(
            written,
            [Written::Observed(LifecycleObservationSource::Intent, true)]
        );
    }

    #[test]
    fn an_unreturned_step_is_settled_with_the_cleanup_it_makes_due() {
        let scenario = Scenario::new(None);
        let (stage, prune) = (scenario.stage(), scenario.prune());
        fs::create_dir_all(
            scenario
                .runtimes()
                .join(scenario.target.runtime_fingerprint()),
        )
        .unwrap();
        let (written, phase, _) = scenario
            .plan(
                stage,
                Some((prune, LifecycleEffectPredicate::PrimaryNotAccepted)),
            )
            .recover()
            .closed("first staging");
        assert_eq!(
            written,
            [
                completed("primary", UNRETURNED),
                observed("primary", true),
                completed("cleanup", NOT_RUN),
                observed("cleanup", true),
            ]
        );
        assert_eq!(phase, LifecycleFailedPhase::NativeDispatch);

        let scenario = Scenario::new(None);
        let bootstrap = LifecycleEffect::BootstrapService {
            target: scenario.target.clone(),
        };
        let unload = LifecycleEffect::UnloadService {
            service: scenario.target.service().into(),
        };
        let (written, _, message) = scenario
            .plan(
                bootstrap,
                Some((unload, LifecycleEffectPredicate::PrimaryNotAccepted)),
            )
            .recover()
            .closed("bootstrap without its target");
        assert_eq!(
            written,
            [
                completed("primary", UNRETURNED),
                observed("primary", false),
                completed("cleanup", NOT_RUN),
                observed("cleanup", false),
            ]
        );
        assert!(message.contains("nothing was booted out"), "{message}");

        let scenario = Scenario::new(None).published();
        let retired = ReconciliationIncarnation::new(
            ReconciliationTarget::new(
                scenario.target.service().into(),
                "c".repeat(64),
                "d".repeat(64),
            )
            .unwrap(),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            4242,
            7398,
        )
        .unwrap();
        let rows = [
            (
                "publication",
                LifecycleEffect::PublishServiceDefinition {
                    target: scenario.target.clone(),
                },
                true,
            ),
            ("pruning", scenario.prune(), false),
            (
                "staging cleanup",
                LifecycleEffect::RemoveStagingRuntime {
                    generation: "d".repeat(64),
                },
                false,
            ),
        ];
        for (row, primary, artifact) in rows {
            let scenario = Scenario {
                home: tempfile::tempdir().unwrap(),
                ..Scenario::new(None)
            };
            let scenario = if row == "publication" {
                scenario.published()
            } else {
                scenario
            };
            let (written, _, _) = scenario.plan(primary, None).recover().closed(row);
            assert_eq!(
                written,
                [
                    completed("primary", UNRETURNED),
                    observed("primary", artifact)
                ],
                "{row}"
            );
        }

        // Retiring the prior gateway, whose plist is installed.
        let retirement = LifecycleEffect::RequestRetirement {
            incarnation: retired.clone(),
        };
        let (written, _, _) = Scenario::new(Some(retired))
            .published()
            .plan(retirement, None)
            .recover()
            .closed("retirement");
        assert_eq!(
            written,
            [completed("primary", UNRETURNED), observed("primary", true)]
        );
    }

    #[test]
    fn an_unconditional_cleanup_already_journaled_is_not_settled_again() {
        // Journals written before cleanups were conditional declared them
        // always due, so the live path could journal one before its primary.
        let scenario = Scenario::new(None);
        let (stage, prune) = (scenario.stage(), scenario.prune());
        let (written, _, _) = scenario
            .plan(
                stage.clone(),
                Some((prune.clone(), LifecycleEffectPredicate::Always)),
            )
            .completed("cleanup", LifecycleCommandResult::Accepted)
            .observed("cleanup", 1, false)
            .recover()
            .closed("cleanup observed, primary unreturned");
        assert_eq!(
            written,
            [completed("primary", UNRETURNED), observed("primary", false)]
        );

        let scenario = Scenario::new(None);
        let (written, _, _) = scenario
            .plan(stage, Some((prune, LifecycleEffectPredicate::Always)))
            .completed("cleanup", LifecycleCommandResult::Accepted)
            .recover()
            .closed("cleanup awaiting its observation, primary unreturned");
        assert_eq!(
            written,
            [
                observed("cleanup", false),
                completed("primary", UNRETURNED),
                observed("primary", false),
            ]
        );
    }

    #[test]
    fn a_bootstrap_whose_exact_target_runs_is_adopted() {
        let scenario = Scenario::new(None).target_running(42);
        let bootstrap = LifecycleEffect::BootstrapService {
            target: scenario.target.clone(),
        };
        let unload = LifecycleEffect::UnloadService {
            service: scenario.target.service().into(),
        };
        let written = scenario
            .plan(
                bootstrap,
                Some((unload, LifecycleEffectPredicate::PrimaryNotAccepted)),
            )
            .recover()
            .adopted("exact bootstrap");
        // Unreturned, so recorded indeterminate; that makes the bootout owed,
        // and recovery records it as not run rather than stopping the gateway.
        assert_eq!(
            written,
            [
                completed("primary", UNRETURNED),
                observed("primary", true),
                completed("cleanup", NOT_RUN),
                observed("cleanup", true),
            ]
        );
    }

    #[test]
    fn an_unreturned_unload_is_settled_without_running_it() {
        for (row, scenario) in [
            ("label still loaded", Scenario::new(None).label_loaded()),
            ("target running", Scenario::new(None).target_running(42)),
            ("absent", Scenario::new(None)),
        ] {
            let loaded = scenario.loaded;
            let unload = LifecycleEffect::UnloadService {
                service: scenario.target.service().into(),
            };
            let (written, phase, message) = scenario.plan(unload, None).recover().closed(row);
            assert_eq!(
                written,
                [
                    completed("primary", UNRETURNED),
                    observed("primary", loaded)
                ],
                "{row}"
            );
            assert_eq!(phase, LifecycleFailedPhase::NativeDispatch, "{row}");
            assert!(message.contains("without replaying"), "{row}: {message}");
        }
    }

    #[test]
    fn a_readiness_step_is_adopted_only_for_its_exact_target() {
        let scenario = Scenario::new(None).target_running(42);
        let adopt = LifecycleEffect::AdoptReadyIncarnation {
            target: scenario.target.clone(),
        };
        let written = scenario
            .plan(adopt.clone(), None)
            .recover()
            .adopted("ready");
        assert_eq!(
            written,
            [
                Written::Completed("primary".into(), LifecycleCommandResult::Accepted),
                observed("primary", true),
            ]
        );
        for (row, scenario) in [
            ("not ready", Scenario::new(None).label_loaded()),
            (
                "another runtime running",
                Scenario::new(None).other_running(43),
            ),
        ] {
            let (written, phase, _) = scenario.plan(adopt.clone(), None).recover().closed(row);
            assert_eq!(phase, LifecycleFailedPhase::NativeDispatch, "{row}");
            assert_eq!(
                written,
                [
                    Written::Completed(
                        "primary".into(),
                        LifecycleCommandResult::Failed(
                            "The planned gateway incarnation was not ready at recovery".into()
                        )
                    ),
                    observed("primary", true),
                ],
                "{row}"
            );
        }
    }

    /// Registration progress over a journal applying the domain's rules; it
    /// can refuse one delivery to model a journal write that failed.
    struct JournalProgress {
        journal: DomainJournal,
        target: ReconciliationTarget,
        refuse: Option<&'static str>,
    }

    impl JournalProgress {
        fn new(target: &ReconciliationTarget, refuse: Option<&'static str>) -> Self {
            let record = LifecycleRecord::new(
                target.service().into(),
                correlation(2),
                0,
                LifecycleRecordPayload::Intent {
                    request_correlation: correlation(1),
                    cause: ReconciliationCause::Startup,
                    initiator: ReconciliationInitiator::DesktopHost,
                    target: target.clone(),
                    before: None,
                },
            )
            .unwrap();
            Self {
                journal: DomainJournal {
                    namespace: target.service().into(),
                    history: Mutex::new(
                        LifecycleHistory::restore(ServiceManager::Launchd, &[record]).unwrap(),
                    ),
                    written: Mutex::new(Vec::new()),
                },
                target: target.clone(),
                refuse,
            }
        }

        fn refused(&self, what: &str) -> Result<(), GatewayError> {
            match self.refuse {
                Some(refused) if refused == what => Err(GatewayError::Registration(format!(
                    "journal refused {what}"
                ))),
                _ => Ok(()),
            }
        }

        fn written(&self) -> Vec<String> {
            self.journal
                .written
                .lock()
                .unwrap()
                .iter()
                .map(|payload| match payload {
                    LifecycleRecordPayload::EffectPlan { plan_id, .. } => format!("plan:{plan_id}"),
                    LifecycleRecordPayload::EffectCompletion {
                        step_id, result, ..
                    } => format!(
                        "{step_id}:{}",
                        match result {
                            LifecycleCommandResult::Accepted => "accepted",
                            LifecycleCommandResult::Rejected(_) => "rejected",
                            LifecycleCommandResult::Failed(_) => "failed",
                            LifecycleCommandResult::Indeterminate(_) => "indeterminate",
                        }
                    ),
                    LifecycleRecordPayload::Observation { source, state } => format!(
                        "observed:{}:{}",
                        match source {
                            LifecycleObservationSource::Effect { step_id, .. } => step_id.as_str(),
                            LifecycleObservationSource::Intent => "intent",
                        },
                        state.target_artifact_present()
                    ),
                    other => format!("{other:?}"),
                })
                .collect()
        }

        fn settled(&self) -> bool {
            self.journal
                .history
                .lock()
                .unwrap()
                .unsettled_steps()
                .is_empty()
        }
    }

    impl GatewayReconciliationProgress for JournalProgress {
        fn readiness_invalidated(&self) {}
        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            unreachable!("the intent is already journaled")
        }
        fn history_observed(&self, _: ReconciliationHistoryFact) {}
        fn effect_planned(
            &self,
            plan_id: &str,
            primary: &LifecyclePlanStep,
            cleanup: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.journal.append(LifecycleRecordPayload::EffectPlan {
                plan_id: plan_id.into(),
                expected_before: None,
                target: self.target.clone(),
                primary: primary.clone(),
                cleanup: cleanup.to_vec(),
            })
        }
        fn effect_completed(
            &self,
            plan_id: &str,
            step_id: &str,
            result: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.refused(&format!("complete:{step_id}"))?;
            self.journal.effect_completion(plan_id, step_id, result)
        }
        fn physical_observed(
            &self,
            source: &LifecycleObservationSource,
            incarnation: Option<ReconciliationIncarnation>,
            target_artifact_present: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            if let LifecycleObservationSource::Effect { step_id, .. } = source {
                self.refused(&format!("observe:{step_id}"))?;
            }
            let version = self
                .journal
                .history
                .lock()
                .unwrap()
                .latest_observation()
                .map_or(1, |prior| prior.version() + 1);
            let observation =
                LifecycleObservation::new(version, incarnation, target_artifact_present);
            self.journal.observation(source, &observation)?;
            Ok(observation)
        }
    }

    fn bootstrap_with(
        exit: i32,
        after: FakeState,
        refuse: Option<&'static str>,
    ) -> (Result<(), RegisterFailure>, JournalProgress, usize) {
        let scenario = Scenario::new(None);
        let mut launchctl = FakeLaunchctl::new(&scenario.target, FakeState::default());
        launchctl.commands = true;
        launchctl.bootstrap = Some((exit, after));
        let progress = JournalProgress::new(&scenario.target, refuse);
        let artifacts = LaunchdArtifacts::for_label(scenario.home.path(), &scenario.label);
        let result = bootstrap_service(
            &progress,
            &launchctl,
            &NeverDisabled,
            BootstrapRequest {
                domain: "gui/501",
                label: &scenario.label,
                service: scenario.target.service(),
                artifacts: &artifacts,
                port: 7398,
                target: &scenario.target,
            },
        );
        let bootouts = *launchctl.bootouts.lock().unwrap();
        (result, progress, bootouts)
    }

    struct NeverDisabled;

    impl DisabledServiceStatus for NeverDisabled {
        fn is_disabled(&self, _: &str, _: &str) -> Result<bool, String> {
            Ok(false)
        }
    }

    fn running_target() -> FakeState {
        FakeState {
            loaded: true,
            running: Some(42),
            running_fingerprint: None,
        }
    }

    #[test]
    fn a_bootstrap_that_succeeds_leaves_nothing_owed() {
        let (result, progress, bootouts) = bootstrap_with(0, running_target(), None);
        assert!(result.is_ok());
        assert_eq!(bootouts, 0);
        assert_eq!(
            progress.written(),
            [
                "plan:bootstrap-service",
                "primary:accepted",
                "observed:primary:true"
            ]
        );
        assert!(progress.settled());
    }

    #[test]
    fn a_refused_bootstrap_records_its_owed_bootout_before_failing() {
        // Nothing loaded: the bootout has nothing to stop and says so.
        let (result, progress, bootouts) = bootstrap_with(5, FakeState::default(), None);
        assert!(matches!(result, Err(RegisterFailure::Physical(_))));
        assert_eq!(bootouts, 0);
        assert_eq!(
            progress.written(),
            [
                "plan:bootstrap-service",
                "primary:rejected",
                "observed:primary:false",
                "unload-bootstrapped-service:indeterminate",
                "observed:unload-bootstrapped-service:false",
            ]
        );
        assert!(progress.settled());

        // launchd refused, yet the exact target runs: the owed bootout stops
        // it and is recorded.
        let (result, progress, bootouts) = bootstrap_with(5, running_target(), None);
        assert!(result.is_err());
        assert_eq!(bootouts, 1);
        assert_eq!(
            progress.written()[3..],
            [
                "unload-bootstrapped-service:accepted",
                "observed:unload-bootstrapped-service:false",
            ]
        );
        assert!(progress.settled());

        // The label is loaded, but not by the exact healthy target (another
        // runtime, or the target not answering yet): the bootout refuses and
        // records that refusal, and launchd's own diagnosis is kept.
        for (row, state) in [
            (
                "another runtime",
                FakeState {
                    running_fingerprint: Some("c".repeat(64)),
                    ..running_target()
                },
            ),
            (
                "target not answering",
                FakeState {
                    loaded: true,
                    ..FakeState::default()
                },
            ),
        ] {
            let (result, progress, bootouts) = bootstrap_with(5, state, None);
            let Err(RegisterFailure::Physical(message)) = result else {
                panic!("{row}: a refused bootstrap fails");
            };
            assert!(
                message.contains("launchd reports a loaded service"),
                "{row}: {message}"
            );
            assert!(message.contains("bootout refused"), "{row}: {message}");
            assert_eq!(bootouts, 0, "{row}");
            assert_eq!(
                progress.written()[3..],
                [
                    "unload-bootstrapped-service:rejected",
                    "observed:unload-bootstrapped-service:true",
                ],
                "{row}"
            );
            assert!(progress.settled(), "{row}");
        }
    }

    #[test]
    fn a_bootstrap_whose_record_fails_is_not_undone() {
        for refuse in ["complete:primary", "observe:primary"] {
            let (result, progress, bootouts) = bootstrap_with(0, running_target(), Some(refuse));
            assert!(matches!(result, Err(RegisterFailure::Audit(_))), "{refuse}");
            assert_eq!(bootouts, 0, "{refuse}");
            assert!(
                !progress
                    .written()
                    .iter()
                    .any(|record| record.starts_with("unload-bootstrapped-service")),
                "{refuse}"
            );
        }
    }

    #[test]
    fn journals_stuck_before_this_fix_recover_without_running_anything() {
        // The #210 shape on disk: an always-owed prune left pending after a
        // staging that succeeded and was observed.
        let scenario = Scenario::new(None);
        let (stage, prune) = (scenario.stage(), scenario.prune());
        let (written, phase, _) = scenario
            .plan(stage, Some((prune, LifecycleEffectPredicate::Always)))
            .completed("primary", LifecycleCommandResult::Accepted)
            .observed("primary", 1, true)
            .recover()
            .closed("stuck staging");
        assert_eq!(
            written,
            [completed("cleanup", NOT_RUN), observed("cleanup", false)]
        );
        assert_eq!(phase, LifecycleFailedPhase::NativeDispatch);

        // An always-owed bootout left pending after a bootstrap that succeeded:
        // the healthy gateway is neither adopted here nor booted out.
        let scenario = Scenario::new(None).target_running(42);
        let bootstrap = LifecycleEffect::BootstrapService {
            target: scenario.target.clone(),
        };
        let unload = LifecycleEffect::UnloadService {
            service: scenario.target.service().into(),
        };
        let (written, _, _) = scenario
            .plan(bootstrap, Some((unload, LifecycleEffectPredicate::Always)))
            .completed("primary", LifecycleCommandResult::Accepted)
            .observed("primary", 1, true)
            .recover()
            .closed("stuck bootstrap");
        assert_eq!(
            written,
            [completed("cleanup", NOT_RUN), observed("cleanup", true)]
        );
    }

    #[test]
    fn a_bootstrap_launchd_refused_is_never_adopted() {
        let scenario = Scenario::new(None).target_running(42);
        let bootstrap = LifecycleEffect::BootstrapService {
            target: scenario.target.clone(),
        };
        let unload = LifecycleEffect::UnloadService {
            service: scenario.target.service().into(),
        };
        let (written, _, message) = scenario
            .plan(
                bootstrap,
                Some((unload, LifecycleEffectPredicate::PrimaryNotAccepted)),
            )
            .completed(
                "primary",
                LifecycleCommandResult::Rejected("Bootstrap failed: 5".into()),
            )
            .recover()
            .closed("refused, exact target running");
        assert_eq!(
            written,
            [
                observed("primary", true),
                completed("cleanup", NOT_RUN),
                observed("cleanup", true),
            ]
        );
        assert!(message.contains("nothing was booted out"), "{message}");
    }

    #[test]
    fn observations_mean_the_target_itself_and_the_file_itself() {
        // No plan while the running gateway is the prior: nothing of this
        // attempt is present.
        let scenario = Scenario::new(None).target_running(42);
        let prior = ReconciliationIncarnation::new(
            scenario.target.clone(),
            "550e8400-e29b-41d4-a716-446655440001".into(),
            42,
            7398,
        )
        .unwrap();
        let (written, _, _) = Scenario {
            before: Some(prior.clone()),
            records: vec![LifecycleRecordPayload::Intent {
                request_correlation: correlation(1),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
                target: scenario.target.clone(),
                before: Some(prior),
            }],
            ..scenario
        }
        .recover()
        .closed("prior is the target");
        assert_eq!(
            written,
            [Written::Observed(LifecycleObservationSource::Intent, false)]
        );

        // Publication means the plist, not whether launchd has the label.
        let scenario = Scenario::new(None).label_loaded();
        let publish = LifecycleEffect::PublishServiceDefinition {
            target: scenario.target.clone(),
        };
        let (written, _, _) = scenario
            .plan(publish, None)
            .recover()
            .closed("loaded, no plist");
        assert_eq!(
            written,
            [completed("primary", UNRETURNED), observed("primary", false)]
        );

        // Staging cleanup means its staging directory.
        let scenario = Scenario::new(None);
        fs::create_dir_all(
            scenario
                .runtimes()
                .join(format!(".staging-{}", "d".repeat(64))),
        )
        .unwrap();
        let (written, _, _) = scenario
            .plan(
                LifecycleEffect::RemoveStagingRuntime {
                    generation: "d".repeat(64),
                },
                None,
            )
            .recover()
            .closed("staging directory present");
        assert_eq!(
            written,
            [completed("primary", UNRETURNED), observed("primary", true)]
        );
    }

    fn unload_with(
        state: FakeState,
        bootout_failure: Option<&str>,
    ) -> (Result<(), RegisterFailure>, JournalProgress, usize) {
        let scenario = Scenario::new(None);
        let mut launchctl = FakeLaunchctl::new(&scenario.target, state);
        launchctl.commands = true;
        launchctl.bootout_failure = bootout_failure.map(str::to_owned);
        let progress = JournalProgress::new(&scenario.target, None);
        let artifacts = LaunchdArtifacts::for_label(scenario.home.path(), &scenario.label);
        let result = unload_service(
            &progress,
            &launchctl,
            &artifacts,
            "unload-stale-service",
            scenario.target.service(),
            7398,
        );
        let bootouts = *launchctl.bootouts.lock().unwrap();
        (result, progress, bootouts)
    }

    #[test]
    fn an_unload_boots_out_the_label_and_records_what_launchd_shows_after() {
        let (result, progress, bootouts) = unload_with(running_target(), None);
        assert!(result.is_ok());
        assert_eq!(bootouts, 1);
        assert_eq!(
            progress.written(),
            [
                "plan:unload-stale-service",
                "primary:accepted",
                "observed:primary:false",
            ]
        );
        assert!(progress.settled());

        // launchd refuses the bootout: the failure is recorded, with the label
        // still loaded.
        let (result, progress, bootouts) =
            unload_with(running_target(), Some("Boot-out failed: 5"));
        let Err(RegisterFailure::Physical(message)) = result else {
            panic!("a refused bootout fails the step");
        };
        assert!(message.contains("Boot-out failed"), "{message}");
        assert_eq!(bootouts, 1);
        assert_eq!(
            progress.written(),
            [
                "plan:unload-stale-service",
                "primary:failed",
                "observed:primary:true",
            ]
        );
    }

    #[test]
    fn readiness_is_read_through_launchctl() {
        let scenario = Scenario::new(None);
        let launchctl = FakeLaunchctl::new(&scenario.target, running_target());
        let log = scenario.home.path().join("logs/gateway.log");
        let running = control::wait_fingerprint(
            &launchctl,
            scenario.target.service(),
            (
                scenario.target.runtime_fingerprint(),
                scenario.target.service_generation(),
            ),
            7398,
            &log,
        )
        .unwrap();
        assert_eq!(running.pid, 42);
    }

    #[test]
    fn the_retirement_signal_goes_through_launchctl() {
        let scenario = Scenario::new(None);
        let mut launchctl = FakeLaunchctl::new(&scenario.target, running_target());
        launchctl.commands = true;
        launchctl.signal_result = Some(LifecycleCommandResult::Rejected(
            "Could not find service".into(),
        ));
        let data = scenario.home.path().join("data");
        let target_generation = "d".repeat(64);
        let running = "c".repeat(64);
        let refused = control::retire(
            &launchctl,
            &data,
            control::Retirement {
                service: scenario.target.service(),
                target: scenario.target.runtime_fingerprint(),
                running: &running,
                instance: "550e8400-e29b-41d4-a716-446655440001",
                running_generation: scenario.target.service_generation(),
                target_generation: &target_generation,
            },
        )
        .unwrap_err();
        assert!(refused.contains("Could not find service"), "{refused}");
        assert_eq!(*launchctl.signals.lock().unwrap(), ["SIGUSR2"]);
    }

    #[test]
    fn a_namespace_this_host_does_not_own_stays_unresolved() {
        let mut scenario = Scenario::new(None);
        scenario.target = ReconciliationTarget::new(
            "gui/0/com.example.other".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap();
        scenario.records = vec![LifecycleRecordPayload::Intent {
            request_correlation: correlation(1),
            cause: ReconciliationCause::Startup,
            initiator: ReconciliationInitiator::DesktopHost,
            target: scenario.target.clone(),
            before: None,
        }];
        let recovered = scenario.recover();
        assert!(recovered.result.is_err());
        assert!(!recovered.terminal);
        assert!(recovered.written.is_empty());
    }
}
