//! Reconciles one packaged gateway with the account's systemd user manager.

use super::{
    paths::LinuxGatewayPaths,
    process::{verify_pidfd_support, LinuxSignalAuthority},
    staging::{
        bytes_match, create_owned_directory_chain, publish_bytes, publish_runtime,
        publish_wants_link, remove_staging_runtime, runtime_fingerprint, staging_runtime,
        wants_link_matches,
    },
    unit::{render, unit_name, RenderedUnit, UnitDefinition},
    user_manager::{verify_linger, JobTerminal, UnitSnapshot, UserManager},
};
use crate::gateway::{
    application::{
        GatewayError, GatewayHost, GatewayLifecycleRecovery, GatewayReconciliationAttempt,
        GatewayReconciliationIntent, GatewayReconciliationJournalSession,
        GatewayReconciliationProgress, GatewayStopSession, MonotonicClock, ReconciledGateway,
        ReconciliationHistoryFact,
    },
    domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, LifecycleEffect, LifecycleEffectPredicate,
        LifecycleFailedPhase, LifecycleObservation, LifecycleObservationSource,
        LifecyclePhysicalOutcome, LifecyclePlanStep, ReconciliationCause,
        ReconciliationCleanupDecision, ReconciliationIncarnation, ReconciliationTarget, SearchPath,
        ServiceConfiguration, SystemdJobMode, SystemdJobOperation, SystemdRuntimeObservation,
        SystemdUnitName,
    },
};
use nessa_gateway_endpoint::{
    domain::GatewayEndpointAdvertisement, infrastructure::FileEndpointDiscovery,
};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const JOB_TIMEOUT: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(45);

pub(crate) struct SystemdGateway {
    configuration: ServiceConfiguration,
    home: PathBuf,
    config_home: Option<OsString>,
    data_home: Option<OsString>,
    clock: Arc<dyn MonotonicClock>,
}

impl SystemdGateway {
    pub fn new(
        configuration: ServiceConfiguration,
        home: PathBuf,
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self {
            configuration,
            home,
            config_home: std::env::var_os("XDG_CONFIG_HOME"),
            data_home: std::env::var_os("XDG_DATA_HOME"),
            clock,
        }
    }

    fn paths(&self, unit: &SystemdUnitName) -> Result<LinuxGatewayPaths, GatewayError> {
        LinuxGatewayPaths::new(
            &self.home,
            self.config_home.clone(),
            self.data_home.clone(),
            unit,
        )
        .map_err(GatewayError::Registration)
    }
}

impl GatewayHost for SystemdGateway {
    fn startup_cause(&self) -> ReconciliationCause {
        ReconciliationCause::Startup
    }

    fn register(
        &self,
        runtime: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
        attempt: &GatewayReconciliationAttempt,
        progress: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        if stage != self.configuration.stage() {
            return Err(GatewayError::Registration(
                "Gateway service configuration stage changed".into(),
            ));
        }
        let effective_uid = unsafe { libc::geteuid() };
        if effective_uid != unsafe { libc::getuid() } {
            return Err(GatewayError::Registration(
                "The packaged gateway refuses a set-user-ID process".into(),
            ));
        }
        verify_linger(effective_uid).map_err(GatewayError::Registration)?;
        verify_pidfd_support().map_err(GatewayError::Registration)?;
        let unit =
            unit_name(stage, self.configuration.instance()).map_err(GatewayError::Registration)?;
        let paths = self.paths(&unit)?;
        let fingerprint = runtime_fingerprint(runtime).map_err(GatewayError::Registration)?;
        let manager = UserManager::connect(effective_uid).map_err(GatewayError::Registration)?;
        verify_unit_authority(
            &manager.unit_path().map_err(GatewayError::Registration)?,
            &paths,
            &unit,
        )
        .map_err(GatewayError::Registration)?;
        let installed = manager
            .snapshot(&unit)
            .map_err(GatewayError::Registration)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            stage,
            self.configuration.instance(),
        );
        let advertisement = discover_endpoint(&data).map_err(GatewayError::Registration)?;
        let before = portable_incarnation(&unit, installed.as_ref(), advertisement.as_ref())?;
        match (installed.as_ref(), before.as_ref()) {
            (Some(snapshot), Some(prior)) => {
                native_from_snapshot(snapshot, prior.target(), &unit, true)
                    .map_err(GatewayError::Registration)?;
                if snapshot.fragment_path != paths.unit_file.to_string_lossy()
                    || !wants_link_matches(&paths.wants_link, &paths.unit_file)
                        .map_err(GatewayError::Registration)?
                    || manager
                        .get_unit_by_pid(prior.process_id())
                        .map_err(GatewayError::Registration)?
                        != snapshot.object_path
                {
                    return Err(GatewayError::Registration(
                        "The running systemd gateway lacks exact owned unit identity".into(),
                    ));
                }
            }
            (Some(snapshot), None)
                if snapshot.main_process_id != 0
                    || !matches!(snapshot.active_state.as_str(), "inactive" | "failed") =>
            {
                return Err(GatewayError::Registration(
                    "A live or ambiguous systemd unit has no corroborated managed endpoint; it was preserved"
                        .into(),
                ));
            }
            _ => {}
        }
        let staged_runtime = paths.runtime_root.join(&fingerprint);
        let path = chosen_agent_path(agent_path, installed.as_ref(), &staged_runtime)?;
        let generation = reusable_generation(installed.as_ref(), &fingerprint)
            .unwrap_or(random_digest().map_err(GatewayError::Registration)?);
        let target = ReconciliationTarget::new(
            unit.as_str().to_owned(),
            fingerprint.clone(),
            generation.clone(),
        )
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
        let rendered = render(UnitDefinition {
            unit: &unit,
            runtime: &staged_runtime,
            configuration: &self.configuration,
            data: &data,
            home: &self.home,
            agent_path: &path,
            fingerprint: &fingerprint,
            generation: &generation,
        })
        .map_err(GatewayError::Registration)?;

        progress.intent_admitted(GatewayReconciliationIntent::new(
            attempt.clone(),
            target.clone(),
            before.clone(),
        )?)?;

        if let Some(ready) = exact_ready(
            &manager,
            &paths,
            &unit,
            &target,
            &rendered,
            advertisement.as_ref(),
        )? {
            let incarnation = ready.audit_identity()?;
            let native = exact_native(
                &manager,
                &paths,
                &unit,
                &target,
                &rendered,
                Some(&incarnation),
            )?
            .ok_or_else(|| {
                GatewayError::Registration(
                    "The healthy gateway lost exact systemd identity before acknowledgement".into(),
                )
            })?;
            progress.systemd_observed(
                &LifecycleObservationSource::Intent,
                incarnation,
                true,
                native,
            )?;
            return Ok(ready);
        }

        if let Some(prior) = before.as_ref().filter(|prior| prior.target() != &target) {
            progress.readiness_invalidated();
            retire_prior(
                progress,
                &manager,
                &unit,
                prior,
                &target,
                &data,
                self.clock.as_ref(),
            )?;
            progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
            stop_unit(progress, &manager, &unit, &target, self.clock.as_ref())?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
        }

        stage_runtime(
            progress,
            runtime,
            &paths.runtime_root,
            &staged_runtime,
            &fingerprint,
        )?;
        prepare_data_directory(self.configuration.data_root(), &data)
            .map_err(GatewayError::Registration)?;
        planned_physical(
            progress,
            "publish-systemd-unit",
            LifecycleEffect::PublishServiceDefinition {
                target: target.clone(),
            },
            || publish_bytes(&paths.unit_file, &rendered.bytes, 0o600),
            || bytes_match(&paths.unit_file, &rendered.bytes).unwrap_or(false),
        )?;
        progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionPublished);
        progress.history_observed(ReconciliationHistoryFact::ServiceDefinitionDurable);
        planned_physical(
            progress,
            "reload-systemd-manager",
            LifecycleEffect::ReloadSystemdManager {
                manager: manager.identity().clone(),
                unit: unit.clone(),
            },
            || manager.reload(),
            || true,
        )?;
        planned_physical(
            progress,
            "create-systemd-wants-directory",
            LifecycleEffect::CreateSystemdWantsDirectory {
                target: target.clone(),
            },
            || create_owned_directory_chain(&paths.wants_directory),
            || paths.wants_directory.is_dir(),
        )?;
        planned_physical(
            progress,
            "publish-systemd-wants-link",
            LifecycleEffect::PublishSystemdWantsLink {
                target: target.clone(),
            },
            || publish_wants_link(&paths.wants_directory, &paths.wants_link, &paths.unit_file),
            || wants_link_matches(&paths.wants_link, &paths.unit_file).unwrap_or(false),
        )?;
        if manager
            .unit_file_state(&unit)
            .map_err(GatewayError::Registration)?
            != "enabled"
        {
            return Err(GatewayError::Registration(
                "systemd did not confirm the owned wants link as enabled".into(),
            ));
        }

        progress.history_observed(ReconciliationHistoryFact::BootstrapCommandRequested);
        start_unit(progress, &manager, &unit, &target, self.clock.as_ref())?;
        progress.history_observed(ReconciliationHistoryFact::BootstrapCommandCompleted);
        progress.history_observed(ReconciliationHistoryFact::BootstrapCommandSucceeded);

        let ready = wait_ready(
            &manager,
            &paths,
            &unit,
            &target,
            &rendered,
            &data,
            self.clock.as_ref(),
        )?;
        let incarnation = ready.audit_identity()?;
        let native = exact_native(
            &manager,
            &paths,
            &unit,
            &target,
            &rendered,
            Some(&incarnation),
        )?
        .ok_or_else(|| {
            GatewayError::Registration(
                "The ready gateway lost exact systemd identity before acknowledgement".into(),
            )
        })?;
        let step = LifecyclePlanStep::new(
            "primary".into(),
            LifecycleEffect::AdoptReadyIncarnation {
                target: target.clone(),
            },
            LifecycleEffectPredicate::Always,
        )
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
        progress.effect_planned("adopt-systemd-incarnation", &step, &[])?;
        progress.effect_completed(
            "adopt-systemd-incarnation",
            step.id(),
            &LifecycleCommandResult::Accepted,
        )?;
        progress.systemd_observed(
            &LifecycleObservationSource::Effect {
                plan_id: "adopt-systemd-incarnation".into(),
                step_id: step.id().into(),
            },
            incarnation,
            true,
            native,
        )?;
        Ok(ready)
    }

    fn recover(
        &self,
        recovery: &GatewayLifecycleRecovery,
        journal: &dyn GatewayReconciliationJournalSession,
    ) -> Result<(), GatewayError> {
        let unit = SystemdUnitName::parse(recovery.target().service().to_owned())
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        let expected = unit_name(self.configuration.stage(), self.configuration.instance())
            .map_err(GatewayError::Registration)?;
        if unit != expected {
            return Err(GatewayError::Registration(
                "The unresolved systemd unit is outside this desktop namespace".into(),
            ));
        }
        let manager =
            UserManager::connect(unsafe { libc::geteuid() }).map_err(GatewayError::Registration)?;
        let paths = self.paths(&unit)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        let advertisement = discover_endpoint(&data).map_err(GatewayError::Registration)?;
        let snapshot = manager
            .snapshot(&unit)
            .map_err(GatewayError::Registration)?;
        let observed = portable_incarnation(&unit, snapshot.as_ref(), advertisement.as_ref())?;
        let broad_artifact_present = wants_link_matches(&paths.wants_link, &paths.unit_file)
            .unwrap_or(false)
            || paths.unit_file.exists()
            || paths
                .runtime_root
                .join(recovery.target().runtime_fingerprint())
                .exists();
        let observation = if let Some(step) = recovery.pending_step() {
            if step.completion().is_none() {
                let detail = match step.step().effect() {
                    LifecycleEffect::StartSystemdUnit { .. }
                    | LifecycleEffect::StopSystemdUnit { .. }
                        if step.native_attempt().is_some() =>
                    {
                        "Recovered a recorded systemd job after its terminal signal boundary; the command was not replayed"
                    }
                    LifecycleEffect::StartSystemdUnit { .. }
                    | LifecycleEffect::StopSystemdUnit { .. } => {
                        "Recovered a systemd enqueue boundary without a durable returned job identity; the command was not replayed"
                    }
                    _ => {
                        "Recovered a planned native effect by fresh observation; the effect was not replayed"
                    }
                };
                retry(|| {
                    journal.effect_completion(
                        step.plan_id(),
                        step.step().id(),
                        &LifecycleCommandResult::Indeterminate(detail.into()),
                    )
                })?;
            }
            let observation = LifecycleObservation::new(
                recovery
                    .latest_observation()
                    .map_or(1, |value| value.version().saturating_add(1)),
                observed,
                recovery_artifact_present(
                    step.step().effect(),
                    &paths,
                    recovery.target(),
                    snapshot.is_some(),
                ),
            );
            let source = step.source();
            retry(|| journal.observation(&source, &observation))?;
            observation
        } else if let Some(observation) = recovery.latest_observation() {
            if observation.incarnation() != observed.as_ref()
                || observation.target_artifact_present() != broad_artifact_present
            {
                return Err(GatewayError::Registration(
                    "Fresh systemd recovery state disagrees with its last durable observation"
                        .into(),
                ));
            }
            observation.clone()
        } else {
            if recovery.has_effect_plan()
                || observed.as_ref() != recovery.before()
                || broad_artifact_present
            {
                return Err(GatewayError::Registration(
                    "The unresolved systemd lifecycle lacks an exact safe closure".into(),
                ));
            }
            let observation = LifecycleObservation::new(1, observed, false);
            retry(|| journal.observation(&LifecycleObservationSource::Intent, &observation))?;
            observation
        };
        retry(|| {
            journal.physical_outcome(
                &LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::Observation,
                    message: "Recovered the unresolved systemd lifecycle by fresh observation; no native command was replayed".into(),
                },
                Some(&observation),
                ReconciliationCleanupDecision::RetainPrior,
            )
        })?;
        Ok(())
    }

    fn stop_agents(
        &self,
        session: &GatewayStopSession,
        journal: &dyn GatewayReconciliationJournalSession,
        plan: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        let token = session.begin_proof()?;
        let intended = session.request().intended();
        let unit = SystemdUnitName::parse(intended.service().to_owned())
            .map_err(|error| GatewayError::Stop(error.to_string()))?;
        let manager =
            UserManager::connect(unsafe { libc::geteuid() }).map_err(GatewayError::Stop)?;
        let paths = self
            .paths(&unit)
            .map_err(|error| GatewayError::Stop(error.to_string()))?;
        let target = intended.audit_identity()?.target().clone();
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        let advertisement = discover_endpoint(&data).map_err(GatewayError::Stop)?;
        let snapshot = manager
            .snapshot(&unit)
            .map_err(GatewayError::Stop)?
            .ok_or_else(|| GatewayError::Stop("The intended systemd unit is absent".into()))?;
        let native =
            native_from_snapshot(&snapshot, &target, &unit, true).map_err(GatewayError::Stop)?;
        let portable = portable_incarnation(&unit, Some(&snapshot), advertisement.as_ref())?
            .ok_or_else(|| {
                GatewayError::Stop(
                    "The intended systemd process has no matching endpoint advertisement".into(),
                )
            })?;
        if portable != intended.audit_identity()? {
            return Err(GatewayError::Stop(
                "The current systemd endpoint is not the intended gateway incarnation".into(),
            ));
        }
        if snapshot.fragment_path != paths.unit_file.to_string_lossy()
            || snapshot.working_directory != data.to_string_lossy()
            || snapshot.exec_start_ex.len() != 1
            || snapshot.exec_start_ex[0].0 != "/usr/bin/env"
            || snapshot.exec_start_ex[0].2 != ["no-env-expand"]
            || manager
                .get_unit_by_pid(intended.process_id())
                .map_err(GatewayError::Stop)?
                != snapshot.object_path
            || !wants_link_matches(&paths.wants_link, &paths.unit_file)
                .map_err(GatewayError::Stop)?
        {
            return Err(GatewayError::Stop(
                "The intended systemd process lost its unit or enablement identity".into(),
            ));
        }
        let second = manager
            .snapshot(&unit)
            .map_err(GatewayError::Stop)?
            .ok_or_else(|| GatewayError::Stop("The intended systemd unit disappeared".into()))?;
        if second != snapshot {
            return Err(GatewayError::Stop(
                "The intended systemd process changed during proof".into(),
            ));
        }
        let command = LinuxSignalAuthority::open(token, native, portable, 1)?.dispatch(
            session,
            plan,
            libc::SIGUSR1,
        )?;
        session.command_result(command.clone())?;
        retry(|| {
            journal.effect_completion("stop-agents-on-desktop-quit", "signal-agents", &command)
        })?;
        let fresh = manager.snapshot(&unit).map_err(GatewayError::Stop)?;
        let observed = fresh
            .as_ref()
            .map(|snapshot| portable_from_snapshot(&unit, snapshot, Some(intended)))
            .transpose()
            .map(Option::flatten)
            .map_err(GatewayError::Stop)?;
        let observation = LifecycleObservation::new(2, observed, paths.wants_link.exists());
        retry(|| {
            journal.observation(
                &LifecycleObservationSource::Effect {
                    plan_id: "stop-agents-on-desktop-quit".into(),
                    step_id: "signal-agents".into(),
                },
                &observation,
            )
        })?;
        session.fresh_observation(observation.clone())?;
        match command {
            LifecycleCommandResult::Accepted => Ok(observation),
            LifecycleCommandResult::Rejected(message)
            | LifecycleCommandResult::Failed(message)
            | LifecycleCommandResult::Indeterminate(message) => Err(GatewayError::Stop(message)),
        }
    }
}

fn stage_runtime(
    progress: &dyn GatewayReconciliationProgress,
    source: &Path,
    root: &Path,
    destination: &Path,
    fingerprint: &str,
) -> Result<(), GatewayError> {
    let staging_generation = random_digest().map_err(GatewayError::Registration)?;
    let primary = LifecyclePlanStep::new(
        "primary".into(),
        LifecycleEffect::StageRuntime {
            fingerprint: fingerprint.into(),
        },
        LifecycleEffectPredicate::Always,
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let cleanup = LifecyclePlanStep::new(
        "remove-staging-runtime".into(),
        LifecycleEffect::RemoveStagingRuntime {
            generation: staging_generation.clone(),
        },
        LifecycleEffectPredicate::PrimaryReturned,
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let plan_id = "stage-systemd-runtime";
    progress.effect_planned(plan_id, &primary, std::slice::from_ref(&cleanup))?;
    let result = publish_runtime(source, root, fingerprint, &staging_generation).map(|_| ());
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(error) = progress.effect_completed(plan_id, primary.id(), &completion) {
        let _ = remove_staging_runtime(root, &staging_generation);
        return Err(error);
    }
    if let Err(error) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: primary.id().into(),
        },
        None,
        destination.exists(),
    ) {
        let _ = remove_staging_runtime(root, &staging_generation);
        return Err(error);
    }
    let cleanup_result = remove_staging_runtime(root, &staging_generation);
    let cleanup_completion = match &cleanup_result {
        Ok(true) => LifecycleCommandResult::Accepted,
        Ok(false) => LifecycleCommandResult::Indeterminate(
            "The exact staging runtime was already absent after the primary returned".into(),
        ),
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    progress.effect_completed(plan_id, cleanup.id(), &cleanup_completion)?;
    progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: cleanup.id().into(),
        },
        None,
        staging_runtime(root, &staging_generation).exists(),
    )?;
    cleanup_result.map_err(GatewayError::Registration)?;
    result.map_err(GatewayError::Registration)
}

fn planned_physical(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    run: impl FnOnce() -> Result<(), String>,
    present: impl FnOnce() -> bool,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &step, &[])?;
    let result = run();
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    progress.effect_completed(plan_id, step.id(), &completion)?;
    progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: step.id().into(),
        },
        None,
        present(),
    )?;
    result.map_err(GatewayError::Registration)
}

fn recovery_artifact_present(
    effect: &LifecycleEffect,
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    unit_loaded: bool,
) -> bool {
    match effect {
        LifecycleEffect::StageRuntime { fingerprint }
        | LifecycleEffect::PruneRuntime { fingerprint } => {
            paths.runtime_root.join(fingerprint).exists()
        }
        LifecycleEffect::RemoveStagingRuntime { generation } => {
            staging_runtime(&paths.runtime_root, generation).exists()
        }
        LifecycleEffect::PublishServiceDefinition { .. } => paths.unit_file.exists(),
        LifecycleEffect::CreateSystemdWantsDirectory { .. } => paths.wants_directory.is_dir(),
        LifecycleEffect::PublishSystemdWantsLink { .. } => {
            wants_link_matches(&paths.wants_link, &paths.unit_file).unwrap_or(false)
        }
        LifecycleEffect::ReloadSystemdManager { .. }
        | LifecycleEffect::StartSystemdUnit { .. }
        | LifecycleEffect::StopSystemdUnit { .. }
        | LifecycleEffect::UnloadService { .. }
        | LifecycleEffect::BootstrapService { .. }
        | LifecycleEffect::AdoptReadyIncarnation { .. } => unit_loaded,
        LifecycleEffect::RequestRetirement { incarnation } => {
            incarnation.target() == target && unit_loaded
        }
        LifecycleEffect::StopAgents { .. } => unit_loaded,
    }
}

fn start_unit(
    progress: &dyn GatewayReconciliationProgress,
    manager: &UserManager,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    clock: &dyn MonotonicClock,
) -> Result<(), GatewayError> {
    run_systemd_job(
        progress,
        "start-systemd-unit",
        LifecycleEffect::StartSystemdUnit {
            manager: manager.identity().clone(),
            unit: unit.clone(),
            mode: SystemdJobMode::Fail,
        },
        manager,
        unit,
        target,
        SystemdJobOperation::Start,
        clock,
    )
}

fn stop_unit(
    progress: &dyn GatewayReconciliationProgress,
    manager: &UserManager,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    clock: &dyn MonotonicClock,
) -> Result<(), GatewayError> {
    run_systemd_job(
        progress,
        "stop-systemd-unit",
        LifecycleEffect::StopSystemdUnit {
            manager: manager.identity().clone(),
            unit: unit.clone(),
            mode: SystemdJobMode::Fail,
        },
        manager,
        unit,
        target,
        SystemdJobOperation::Stop,
        clock,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_systemd_job(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    manager: &UserManager,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    operation: SystemdJobOperation,
    clock: &dyn MonotonicClock,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &step, &[])?;
    let handle = match manager.enqueue(operation, unit, clock) {
        Ok(handle) => handle,
        Err(error) => {
            progress.effect_completed(
                plan_id,
                step.id(),
                &LifecycleCommandResult::Indeterminate(error.clone()),
            )?;
            progress.physical_observed(
                &LifecycleObservationSource::Effect {
                    plan_id: plan_id.into(),
                    step_id: step.id().into(),
                },
                None,
                false,
            )?;
            return Err(GatewayError::Registration(error));
        }
    };
    progress.native_attempt_recorded(plan_id, step.id(), &handle.attempt)?;
    let terminal = handle.wait(clock, clock.now() + JOB_TIMEOUT);
    let completion = classify_terminal(manager, unit, &terminal);
    progress.effect_completed(plan_id, step.id(), &completion)?;
    let snapshot = manager.snapshot(unit).map_err(GatewayError::Registration)?;
    let incarnation = snapshot
        .as_ref()
        .map(|snapshot| portable_from_snapshot(unit, snapshot, None))
        .transpose()
        .map(Option::flatten)
        .map_err(GatewayError::Registration)?;
    progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: step.id().into(),
        },
        incarnation,
        snapshot.is_some(),
    )?;
    if operation == SystemdJobOperation::Stop
        && snapshot.as_ref().is_some_and(|state| {
            state.main_process_id != 0
                || !matches!(state.active_state.as_str(), "inactive" | "failed")
        })
    {
        return Err(GatewayError::Registration(
            "systemd completed the stop job but fresh unit state still owns a process".into(),
        ));
    }
    match completion {
        LifecycleCommandResult::Accepted => Ok(()),
        LifecycleCommandResult::Rejected(message)
        | LifecycleCommandResult::Failed(message)
        | LifecycleCommandResult::Indeterminate(message) => {
            Err(GatewayError::Registration(format!(
                "systemd operation for {} failed: {message}",
                target.service()
            )))
        }
    }
}

fn classify_terminal(
    manager: &UserManager,
    unit: &SystemdUnitName,
    terminal: &Result<JobTerminal, String>,
) -> LifecycleCommandResult {
    if let Err(error) = manager.recheck_identity() {
        return LifecycleCommandResult::Indeterminate(error);
    }
    match terminal {
        Ok(terminal)
            if terminal.manager == *manager.identity()
                && terminal.unit == *unit
                && terminal.result == "done" =>
        {
            LifecycleCommandResult::Accepted
        }
        Ok(terminal) => LifecycleCommandResult::Rejected(format!(
            "job {} at {} completed with {}",
            terminal.job_id, terminal.object_path, terminal.result
        )),
        Err(error) => LifecycleCommandResult::Indeterminate(error.clone()),
    }
}

fn exact_ready(
    manager: &UserManager,
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    rendered: &RenderedUnit,
    advertisement: Option<&GatewayEndpointAdvertisement>,
) -> Result<Option<ReconciledGateway>, GatewayError> {
    let snapshot = manager.snapshot(unit).map_err(GatewayError::Registration)?;
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    if !snapshot_matches(&snapshot, manager, paths, unit, rendered)? {
        return Ok(None);
    }
    let Some(advertisement) = advertisement else {
        return Ok(None);
    };
    gateway_from_evidence(unit, target, &snapshot, advertisement).map(Some)
}

fn wait_ready(
    manager: &UserManager,
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    rendered: &RenderedUnit,
    data: &Path,
    clock: &dyn MonotonicClock,
) -> Result<ReconciledGateway, GatewayError> {
    let deadline = clock.now() + READY_TIMEOUT;
    loop {
        let advertisement = discover_endpoint(data).map_err(GatewayError::Registration)?;
        if let Some(ready) = exact_ready(
            manager,
            paths,
            unit,
            target,
            rendered,
            advertisement.as_ref(),
        )? {
            return Ok(ready);
        }
        if clock.now() >= deadline {
            return Err(GatewayError::Registration(
                "The systemd gateway did not publish matching readiness before its deadline".into(),
            ));
        }
        clock.wait(Duration::from_millis(100));
    }
}

fn snapshot_matches(
    snapshot: &UnitSnapshot,
    manager: &UserManager,
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    rendered: &RenderedUnit,
) -> Result<bool, GatewayError> {
    manager
        .recheck_identity()
        .map_err(GatewayError::Registration)?;
    let expected_fragment = paths
        .unit_file
        .to_str()
        .ok_or_else(|| GatewayError::Registration("The unit path is not UTF-8".into()))?;
    Ok(snapshot.manager == *manager.identity()
        && snapshot.id == unit.as_str()
        && snapshot.names == [unit.as_str()]
        && snapshot.fragment_path == expected_fragment
        && snapshot.drop_in_paths.is_empty()
        && snapshot.active_state == "active"
        && snapshot.sub_state == "running"
        && snapshot.invocation.is_some()
        && snapshot.main_process_id != 0
        && snapshot.working_directory == rendered.working_directory
        && snapshot.environment.is_empty()
        && snapshot.exec_start_ex.len() == 1
        && snapshot.exec_start_ex[0].0 == "/usr/bin/env"
        && snapshot.exec_start_ex[0].1 == rendered.arguments
        && snapshot.exec_start_ex[0].2 == ["no-env-expand"]
        && snapshot.unit_file_state == "enabled"
        && snapshot.unit_path == manager.unit_path().map_err(GatewayError::Registration)?
        && bytes_match(&paths.unit_file, &rendered.bytes).map_err(GatewayError::Registration)?
        && wants_link_matches(&paths.wants_link, &paths.unit_file)
            .map_err(GatewayError::Registration)?)
}

fn gateway_from_evidence(
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    snapshot: &UnitSnapshot,
    advertisement: &GatewayEndpointAdvertisement,
) -> Result<ReconciledGateway, GatewayError> {
    let managed = advertisement.managed().ok_or_else(|| {
        GatewayError::Registration("The gateway endpoint lacks managed runtime identity".into())
    })?;
    if managed.fingerprint() != target.runtime_fingerprint()
        || managed.generation() != target.service_generation()
        || managed.endpoint().process_id() != snapshot.main_process_id
        || advertisement.endpoint().identity().process_id() != snapshot.main_process_id
    {
        return Err(GatewayError::Registration(
            "The systemd process and gateway endpoint identities disagree".into(),
        ));
    }
    Ok(ReconciledGateway::new(
        unit.as_str().to_owned(),
        managed.fingerprint().to_owned(),
        managed.endpoint().instance().to_owned(),
        managed.generation().to_owned(),
        snapshot.main_process_id,
        endpoint_port(advertisement.endpoint().web_socket_url().as_str())?,
    ))
}

fn exact_native(
    manager: &UserManager,
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    rendered: &RenderedUnit,
    incarnation: Option<&ReconciliationIncarnation>,
) -> Result<Option<SystemdRuntimeObservation>, GatewayError> {
    let Some(snapshot) = manager.snapshot(unit).map_err(GatewayError::Registration)? else {
        return Ok(None);
    };
    if !snapshot_matches(&snapshot, manager, paths, unit, rendered)? {
        return Ok(None);
    }
    if incarnation.is_some_and(|value| value.process_id() != snapshot.main_process_id) {
        return Ok(None);
    }
    native_from_snapshot(&snapshot, target, unit, true)
        .map(Some)
        .map_err(GatewayError::Registration)
}

fn native_from_snapshot(
    snapshot: &UnitSnapshot,
    target: &ReconciliationTarget,
    unit: &SystemdUnitName,
    enabled: bool,
) -> Result<SystemdRuntimeObservation, String> {
    let arguments = snapshot
        .exec_start_ex
        .first()
        .map(|entry| &entry.1)
        .ok_or_else(|| "The systemd service has no executable identity".to_string())?;
    if snapshot.id != unit.as_str()
        || snapshot.names != [unit.as_str()]
        || !snapshot.drop_in_paths.is_empty()
        || snapshot.active_state != "active"
        || snapshot.sub_state != "running"
        || snapshot.main_process_id == 0
        || snapshot.unit_file_state != "enabled"
        || !snapshot.environment.is_empty()
        || environment_value(arguments, "NESSA_RUNTIME_FINGERPRINT")
            != Some(target.runtime_fingerprint())
        || environment_value(arguments, "NESSA_SERVICE_GENERATION")
            != Some(target.service_generation())
    {
        return Err("The systemd runtime snapshot disagrees with the intended target".into());
    }
    SystemdRuntimeObservation::new(
        target.clone(),
        snapshot.manager.clone(),
        unit.clone(),
        snapshot
            .invocation
            .ok_or_else(|| "The systemd service has no invocation identity".to_string())?,
        snapshot.main_process_id,
        enabled,
    )
    .map_err(|error| error.to_string())
}

fn portable_incarnation(
    unit: &SystemdUnitName,
    snapshot: Option<&UnitSnapshot>,
    advertisement: Option<&GatewayEndpointAdvertisement>,
) -> Result<Option<ReconciliationIncarnation>, GatewayError> {
    let (Some(snapshot), Some(advertisement)) = (snapshot, advertisement) else {
        return Ok(None);
    };
    let Some(managed) = advertisement.managed() else {
        return Ok(None);
    };
    if managed.endpoint().process_id() != snapshot.main_process_id
        || advertisement.endpoint().identity().process_id() != snapshot.main_process_id
    {
        return Ok(None);
    }
    let target = ReconciliationTarget::new(
        unit.as_str().into(),
        managed.fingerprint().into(),
        managed.generation().into(),
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    ReconciliationIncarnation::new(
        target,
        managed.endpoint().instance().into(),
        snapshot.main_process_id,
        endpoint_port(advertisement.endpoint().web_socket_url().as_str())?,
    )
    .map(Some)
    .map_err(|error| GatewayError::Registration(error.to_string()))
}

fn portable_from_snapshot(
    unit: &SystemdUnitName,
    snapshot: &UnitSnapshot,
    fallback: Option<&ReconciledGateway>,
) -> Result<Option<ReconciliationIncarnation>, String> {
    let arguments = snapshot.exec_start_ex.first().map(|entry| &entry.1);
    let fingerprint =
        arguments.and_then(|values| environment_value(values, "NESSA_RUNTIME_FINGERPRINT"));
    let generation =
        arguments.and_then(|values| environment_value(values, "NESSA_SERVICE_GENERATION"));
    let instance = fallback.map(ReconciledGateway::runtime_instance);
    let port = arguments
        .and_then(|values| environment_value(values, "NESSA_PORT"))
        .and_then(|value| value.parse().ok())
        .or_else(|| fallback.map(ReconciledGateway::port));
    let (Some(fingerprint), Some(generation), Some(instance), Some(port)) =
        (fingerprint, generation, instance, port)
    else {
        return Ok(None);
    };
    let target =
        ReconciliationTarget::new(unit.as_str().into(), fingerprint.into(), generation.into())
            .map_err(|error| error.to_string())?;
    ReconciliationIncarnation::new(target, instance.into(), snapshot.main_process_id, port)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn chosen_agent_path(
    supplied: Option<&SearchPath>,
    installed: Option<&UnitSnapshot>,
    staged_runtime: &Path,
) -> Result<SearchPath, GatewayError> {
    let candidate = supplied
        .cloned()
        .or_else(|| {
            installed
                .and_then(|snapshot| snapshot.exec_start_ex.first())
                .and_then(|entry| environment_value(&entry.1, "NESSA_AGENT_PATH"))
                .and_then(|value| SearchPath::parse(value).ok())
        })
        .ok_or_else(|| {
            GatewayError::Registration(
                "The account search path is unavailable and no registered path can be retained"
                    .into(),
            )
        })?;
    candidate.excluding(staged_runtime).ok_or_else(|| {
        GatewayError::Registration("The agent search path only contained the staged runtime".into())
    })
}

fn reusable_generation(snapshot: Option<&UnitSnapshot>, fingerprint: &str) -> Option<String> {
    let arguments = snapshot?.exec_start_ex.first().map(|entry| &entry.1)?;
    (environment_value(arguments, "NESSA_RUNTIME_FINGERPRINT") == Some(fingerprint))
        .then(|| environment_value(arguments, "NESSA_SERVICE_GENERATION"))
        .flatten()
        .filter(|value| digest(value))
        .map(str::to_owned)
}

fn environment_value<'a>(arguments: &'a [String], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    arguments
        .iter()
        .find_map(|argument| argument.strip_prefix(&prefix))
}

fn verify_unit_authority(
    unit_path: &[String],
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
) -> Result<(), String> {
    if unit_path.iter().any(|entry| {
        let path = Path::new(entry);
        !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
    }) {
        return Err("systemd UnitPath contains a noncanonical directory".into());
    }
    let owned = paths
        .unit_root
        .to_str()
        .ok_or_else(|| "The owned systemd unit directory is not UTF-8".to_string())?;
    let position = unit_path
        .iter()
        .position(|candidate| candidate == owned)
        .ok_or_else(|| {
            "The owned systemd user unit directory is absent from UnitPath".to_string()
        })?;
    for directory in &unit_path[..position] {
        if fs::symlink_metadata(Path::new(directory).join(unit.as_str())).is_ok() {
            return Err("A higher-priority systemd unit shadows the owned gateway unit".into());
        }
    }
    Ok(())
}

fn prepare_data_directory(root: &Path, data: &Path) -> Result<(), String> {
    nessa_local_storage::create_directory(root).map_err(|error| error.to_string())?;
    let relative = data
        .strip_prefix(root)
        .map_err(|_| "The gateway data directory escaped its trusted root".to_string())?;
    if !relative.as_os_str().is_empty() {
        nessa_local_storage::create_directory_beneath(root, relative)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn data_directory_path(root: &Path, stage: &str, instance: Option<&str>) -> PathBuf {
    let mut data = root.to_path_buf();
    if stage != "prod" {
        data.push(stage);
    }
    if let Some(instance) = instance {
        data.push("instances");
        data.push(instance);
    }
    data
}

fn endpoint_port(url: &str) -> Result<u16, GatewayError> {
    url.rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| GatewayError::Registration("The gateway endpoint has no port".into()))
}

fn discover_endpoint(data: &Path) -> Result<Option<GatewayEndpointAdvertisement>, String> {
    match FileEndpointDiscovery::new(data.to_path_buf(), "logs".into()).discover_advertisement() {
        Ok(advertisement) => Ok(advertisement),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::NotConnected
                    | ErrorKind::TimedOut
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::WouldBlock
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn random_digest() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn random_uuid() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn retry<T>(mut operation: impl FnMut() -> Result<T, GatewayError>) -> Result<T, GatewayError> {
    operation().or_else(|_| operation())
}

fn retire_prior(
    progress: &dyn GatewayReconciliationProgress,
    manager: &UserManager,
    unit: &SystemdUnitName,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    data: &Path,
    clock: &dyn MonotonicClock,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new(
        "primary".into(),
        LifecycleEffect::RequestRetirement {
            incarnation: prior.clone(),
        },
        LifecycleEffectPredicate::Always,
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned("request-systemd-retirement", &step, &[])?;
    let descriptor = retirement_pidfd_open(prior.process_id());
    if descriptor < 0 {
        return Err(GatewayError::Registration(format!(
            "The retiring gateway process cannot be held: {}",
            std::io::Error::last_os_error()
        )));
    }
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor as i32) };
    let snapshot = manager
        .snapshot(unit)
        .map_err(GatewayError::Registration)?
        .ok_or_else(|| GatewayError::Registration("The retiring systemd unit vanished".into()))?;
    let advertisement = discover_endpoint(data).map_err(GatewayError::Registration)?;
    if portable_incarnation(unit, Some(&snapshot), advertisement.as_ref())?.as_ref() != Some(prior)
        || manager
            .get_unit_by_pid(prior.process_id())
            .map_err(GatewayError::Registration)?
            != snapshot.object_path
    {
        return Err(GatewayError::Registration(
            "The retiring systemd process changed after the effect plan was acknowledged".into(),
        ));
    }
    let result = retire(data, prior, target, &descriptor, clock);
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    progress.effect_completed("request-systemd-retirement", step.id(), &completion)?;
    let snapshot = manager.snapshot(unit).map_err(GatewayError::Registration)?;
    progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: "request-systemd-retirement".into(),
            step_id: step.id().into(),
        },
        snapshot
            .as_ref()
            .map(|snapshot| portable_from_snapshot(unit, snapshot, Some(&gateway(prior))))
            .transpose()
            .map(Option::flatten)
            .map_err(GatewayError::Registration)?,
        snapshot.is_some(),
    )?;
    result.map_err(GatewayError::Registration)
}

fn gateway(value: &ReconciliationIncarnation) -> ReconciledGateway {
    ReconciledGateway::new(
        value.target().service().into(),
        value.target().runtime_fingerprint().into(),
        value.runtime_instance().into(),
        value.target().service_generation().into(),
        value.process_id(),
        value.port(),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetirementResult {
    request_id: String,
    target_fingerprint: String,
    running_fingerprint: String,
    running_instance: String,
    requested_instance: String,
    running_generation: String,
    requested_running_generation: String,
    target_generation: String,
    retired: bool,
    retirement_request_id: Option<String>,
    retirement_cause: Option<RetirementCause>,
    cleanup_error: Option<String>,
    audit_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetirementCause {
    principal_id: String,
    surface_id: String,
    request_id: String,
}

fn retire(
    data: &Path,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    descriptor: &OwnedFd,
    clock: &dyn MonotonicClock,
) -> Result<(), String> {
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory(&directory).map_err(|error| error.to_string())?;
    let request_id = random_uuid()?;
    let request = serde_json::to_vec(&serde_json::json!({
        "requestId": request_id,
        "targetFingerprint": target.runtime_fingerprint(),
        "runningInstance": prior.runtime_instance(),
        "runningGeneration": prior.target().service_generation(),
        "targetGeneration": target.service_generation(),
    }))
    .map_err(|error| error.to_string())?;
    atomic_write(&directory.join("request.json"), &request)?;
    let sent = retirement_pidfd_signal(descriptor.as_raw_fd());
    if sent != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let deadline = clock.now() + Duration::from_secs(75);
    while clock.now() < deadline {
        if retirement_acknowledged(&directory.join("result.json"), &request_id, prior, target)? {
            return Ok(());
        }
        clock.wait(Duration::from_millis(100));
    }
    Err("Gateway retirement acknowledgement timed out; service was preserved".into())
}

#[cfg(target_os = "linux")]
fn retirement_pidfd_open(process_id: u32) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_open,
            libc::pid_t::try_from(process_id).unwrap_or(-1),
            0,
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn retirement_pidfd_open(_process_id: u32) -> libc::c_long {
    -1
}

#[cfg(target_os = "linux")]
fn retirement_pidfd_signal(descriptor: i32) -> libc::c_long {
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            descriptor,
            libc::SIGUSR2,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    }
}

#[cfg(not(target_os = "linux"))]
fn retirement_pidfd_signal(_descriptor: i32) -> libc::c_long {
    -1
}

fn retirement_acknowledged(
    path: &Path,
    request_id: &str,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
) -> Result<bool, String> {
    let file = match nessa_local_storage::open(path, nessa_local_storage::OpenMode::ReadNonblocking)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    (&file)
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Gateway retirement acknowledgement exceeds its limit".into());
    }
    let result: RetirementResult = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid gateway retirement acknowledgement: {error}"))?;
    let cause = result.retirement_cause.as_ref();
    let agrees = result.request_id == request_id
        && result.target_fingerprint == target.runtime_fingerprint()
        && result.running_fingerprint == prior.target().runtime_fingerprint()
        && result.running_instance == prior.runtime_instance()
        && result.requested_instance == prior.runtime_instance()
        && result.running_generation == prior.target().service_generation()
        && result.requested_running_generation == prior.target().service_generation()
        && result.target_generation == target.service_generation()
        && result.retirement_request_id.as_deref() == Some(request_id)
        && cause.is_some_and(|cause| {
            cause.request_id == request_id
                && cause.principal_id == "gateway"
                && cause.surface_id == "gateway_upgrade"
        })
        && result.retired
        && result.cleanup_error.is_none()
        && result.audit_error.is_none();
    if !agrees {
        return Err(
            "Gateway retirement acknowledgement disagrees with the planned identities".into(),
        );
    }
    file.sync_all().map_err(|error| error.to_string())?;
    nessa_local_storage::sync_directory(
        path.parent()
            .ok_or_else(|| "Retirement acknowledgement has no directory".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(true)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "Retirement request has no directory".to_string())?;
    let temporary = directory.join(format!(".nessa-retirement-{}", random_digest()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        nessa_local_storage::replace(&temporary, path).map_err(|error| error.to_string())?;
        nessa_local_storage::sync_directory(directory).map_err(|error| error.to_string())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::domain::value_objects::{SystemdInvocationId, SystemdManagerIdentity};

    fn fixture() -> (SystemdUnitName, ReconciliationTarget, UnitSnapshot) {
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let target =
            ReconciliationTarget::new(unit.as_str().into(), "a".repeat(64), "b".repeat(64))
                .unwrap();
        let arguments = vec![
            "/usr/bin/env".into(),
            "-i".into(),
            format!("NESSA_RUNTIME_FINGERPRINT={}", target.runtime_fingerprint()),
            format!("NESSA_SERVICE_GENERATION={}", target.service_generation()),
            "NESSA_PORT=7420".into(),
        ];
        let snapshot = UnitSnapshot {
            manager: SystemdManagerIdentity::new(":1.41".into(), 41, 501).unwrap(),
            object_path: "/org/freedesktop/systemd1/unit/nessa_2dgateway_2dprod_2eservice".into(),
            id: unit.as_str().into(),
            names: vec![unit.as_str().into()],
            fragment_path: "/home/me/.config/systemd/user/nessa-gateway-prod.service".into(),
            drop_in_paths: vec![],
            active_state: "active".into(),
            sub_state: "running".into(),
            invocation: Some(SystemdInvocationId::new(vec![7; 16]).unwrap()),
            main_process_id: 99,
            working_directory: "/home/me/.nessa".into(),
            environment: vec![],
            exec_start_ex: vec![(
                "/usr/bin/env".into(),
                arguments,
                vec!["no-env-expand".into()],
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            )],
            unit_file_state: "enabled".into(),
            unit_path: vec!["/home/me/.config/systemd/user".into()],
        };
        (unit, target, snapshot)
    }

    #[test]
    fn native_runtime_acceptance_is_table_driven_across_contradictory_fields() {
        let (unit, target, snapshot) = fixture();
        assert!(native_from_snapshot(&snapshot, &target, &unit, true).is_ok());

        let mut cases = Vec::new();
        let mut wrong = snapshot.clone();
        wrong.id = "other.service".into();
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.names.push("alias.service".into());
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.drop_in_paths.push("/tmp/override.conf".into());
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.main_process_id = 0;
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.unit_file_state = "disabled".into();
        cases.push(wrong);
        let mut wrong = snapshot;
        wrong.exec_start_ex[0].1[2] = format!("NESSA_RUNTIME_FINGERPRINT={}", "c".repeat(64));
        cases.push(wrong);

        for contradictory in cases {
            assert!(native_from_snapshot(&contradictory, &target, &unit, true).is_err());
        }
    }

    #[test]
    fn unit_path_refuses_a_higher_priority_definition() {
        let temporary = tempfile::tempdir().unwrap();
        let owned = temporary.path().join("owned");
        let higher = temporary.path().join("higher");
        fs::create_dir_all(&owned).unwrap();
        fs::create_dir_all(&higher).unwrap();
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let paths = LinuxGatewayPaths {
            unit_file: owned.join(unit.as_str()),
            wants_directory: owned.join("default.target.wants"),
            wants_link: owned.join("default.target.wants").join(unit.as_str()),
            runtime_root: temporary.path().join("runtime"),
            unit_root: owned.clone(),
        };
        let search = vec![
            higher.to_string_lossy().into_owned(),
            owned.to_string_lossy().into_owned(),
        ];
        assert!(verify_unit_authority(&search, &paths, &unit).is_ok());
        fs::write(higher.join(unit.as_str()), b"shadow").unwrap();
        assert!(verify_unit_authority(&search, &paths, &unit).is_err());
    }
}
