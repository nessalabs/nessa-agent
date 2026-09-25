//! Reconciles one packaged gateway with the account's systemd user manager.

use super::{
    paths::LinuxGatewayPaths,
    process::{verify_pidfd_support, LinuxSignalAuthority},
    staging::{
        bytes_match, create_owned_directory_chain, definition_transaction, owned_file_bytes,
        publish_bytes, publish_runtime, publish_wants_link, remove_staging_runtime,
        replace_owned_bytes, runtime_fingerprint, settle_definition_transaction,
        staging_runtime_present, validate_runtime, wants_link_matches,
    },
    unit::{render, rendered_agent_path, unit_name, RenderedUnit, UnitDefinition},
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
    fs,
    io::{ErrorKind, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::fs::{MetadataExt, PermissionsExt},
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
    manager_factory: Arc<dyn LinuxManagerFactory>,
    runtime_context: Arc<dyn LinuxRuntimeContext>,
}

trait LinuxManagerFactory: Send + Sync {
    fn connect(&self, expected_uid: u32) -> Result<Box<dyn LinuxUserManager>, String>;
}

trait LinuxRuntimeContext: Send + Sync {
    fn user_ids(&self) -> (u32, u32);
    fn discover_endpoint(
        &self,
        data: &Path,
    ) -> Result<Option<GatewayEndpointAdvertisement>, String>;
}

struct LinuxRuntime<'a> {
    manager: &'a dyn LinuxUserManager,
    context: &'a dyn LinuxRuntimeContext,
    clock: &'a dyn MonotonicClock,
}

struct DefinitionAuthority<'a> {
    configuration: &'a ServiceConfiguration,
    data: &'a Path,
    home: &'a Path,
}

trait LinuxUserManager: Send + Sync {
    fn identity(&self) -> &crate::gateway::domain::value_objects::SystemdManagerIdentity;
    fn recheck_identity(&self) -> Result<(), String>;
    fn unit_path(&self) -> Result<Vec<String>, String>;
    fn environment(&self) -> Result<Vec<String>, String>;
    fn reload(&self) -> Result<(), String>;
    fn unit_file_state(&self, unit: &SystemdUnitName) -> Result<String, String>;
    fn get_unit_by_pid(&self, process_id: u32) -> Result<String, String>;
    fn snapshot(&self, unit: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String>;
    fn enqueue(
        &self,
        operation: SystemdJobOperation,
        unit: &SystemdUnitName,
        clock: &dyn MonotonicClock,
    ) -> Result<super::user_manager::JobHandle, String>;
}

struct NativeLinuxManagerFactory;
struct NativeLinuxRuntimeContext;

impl LinuxManagerFactory for NativeLinuxManagerFactory {
    fn connect(&self, expected_uid: u32) -> Result<Box<dyn LinuxUserManager>, String> {
        verify_linger(expected_uid)?;
        verify_pidfd_support()?;
        UserManager::connect(expected_uid)
            .map(|manager| Box::new(manager) as Box<dyn LinuxUserManager>)
    }
}

impl LinuxRuntimeContext for NativeLinuxRuntimeContext {
    fn user_ids(&self) -> (u32, u32) {
        unsafe { (libc::geteuid(), libc::getuid()) }
    }

    fn discover_endpoint(
        &self,
        data: &Path,
    ) -> Result<Option<GatewayEndpointAdvertisement>, String> {
        discover_endpoint(data)
    }
}

impl LinuxUserManager for UserManager {
    fn identity(&self) -> &crate::gateway::domain::value_objects::SystemdManagerIdentity {
        self.identity()
    }
    fn recheck_identity(&self) -> Result<(), String> {
        self.recheck_identity()
    }
    fn unit_path(&self) -> Result<Vec<String>, String> {
        self.unit_path()
    }
    fn environment(&self) -> Result<Vec<String>, String> {
        self.environment()
    }
    fn reload(&self) -> Result<(), String> {
        self.reload()
    }
    fn unit_file_state(&self, unit: &SystemdUnitName) -> Result<String, String> {
        self.unit_file_state(unit)
    }
    fn get_unit_by_pid(&self, process_id: u32) -> Result<String, String> {
        self.get_unit_by_pid(process_id)
    }
    fn snapshot(&self, unit: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String> {
        self.snapshot(unit)
    }
    fn enqueue(
        &self,
        operation: SystemdJobOperation,
        unit: &SystemdUnitName,
        clock: &dyn MonotonicClock,
    ) -> Result<super::user_manager::JobHandle, String> {
        self.enqueue(operation, unit, clock)
    }
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
            manager_factory: Arc::new(NativeLinuxManagerFactory),
            runtime_context: Arc::new(NativeLinuxRuntimeContext),
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
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid {
            return Err(GatewayError::Registration(
                "The packaged gateway refuses a set-user-ID process".into(),
            ));
        }
        let unit =
            unit_name(stage, self.configuration.instance()).map_err(GatewayError::Registration)?;
        let paths = self.paths(&unit)?;
        let fingerprint = runtime_fingerprint(runtime).map_err(GatewayError::Registration)?;
        let manager = self
            .manager_factory
            .connect(effective_uid)
            .map_err(GatewayError::Registration)?;
        verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home)
            .map_err(GatewayError::Registration)?;
        let installed = manager
            .snapshot(&unit)
            .map_err(GatewayError::Registration)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            stage,
            self.configuration.instance(),
        );
        let advertisement = self
            .runtime_context
            .discover_endpoint(&data)
            .map_err(GatewayError::Registration)?;
        let before = portable_incarnation(&unit, installed.as_ref(), advertisement.as_ref())?;
        let prior_definition = match (installed.as_ref(), before.as_ref()) {
            (Some(snapshot), Some(prior)) => {
                native_from_snapshot(snapshot, prior.target(), &unit, true)
                    .map_err(GatewayError::Registration)?;
                let prior_path = snapshot
                    .exec_start_ex
                    .first()
                    .and_then(|entry| environment_value(&entry.1, "NESSA_AGENT_PATH"))
                    .ok_or_else(|| {
                        GatewayError::Registration(
                            "The running systemd gateway has no retained agent path".into(),
                        )
                    })
                    .and_then(|path| {
                        SearchPath::parse(path).map_err(|error| {
                            GatewayError::Registration(format!(
                                "The running systemd gateway agent path is invalid: {error}"
                            ))
                        })
                    })?;
                let prior_rendered = render(UnitDefinition {
                    unit: &unit,
                    runtime: &paths
                        .runtime_root
                        .join(prior.target().runtime_fingerprint()),
                    configuration: &self.configuration,
                    data: &data,
                    home: &self.home,
                    agent_path: &prior_path,
                    fingerprint: prior.target().runtime_fingerprint(),
                    generation: prior.target().service_generation(),
                })
                .map_err(GatewayError::Registration)?;
                if !snapshot_matches(snapshot, manager.as_ref(), &paths, &unit, &prior_rendered)?
                    || manager
                        .get_unit_by_pid(prior.process_id())
                        .map_err(GatewayError::Registration)?
                        != snapshot.object_path
                {
                    return Err(GatewayError::Registration(
                        "The running systemd gateway lacks exact owned unit identity".into(),
                    ));
                }
                Some(prior_rendered.bytes)
            }
            (Some(_), None) => {
                return Err(GatewayError::Registration(
                    "An installed systemd unit has no corroborated managed endpoint; it was preserved"
                        .into(),
                ));
            }
            _ => None,
        };
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
            manager.as_ref(),
            &paths,
            &unit,
            &target,
            &rendered,
            advertisement.as_ref(),
        )? {
            let incarnation = ready.audit_identity()?;
            let native = exact_native(
                manager.as_ref(),
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
                &unit,
                prior,
                &target,
                &data,
                LinuxRuntime {
                    manager: manager.as_ref(),
                    context: self.runtime_context.as_ref(),
                    clock: self.clock.as_ref(),
                },
            )?;
            progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
            stop_unit(
                progress,
                &unit,
                &target,
                &data,
                LinuxRuntime {
                    manager: manager.as_ref(),
                    context: self.runtime_context.as_ref(),
                    clock: self.clock.as_ref(),
                },
            )?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
        }

        verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home)
            .map_err(GatewayError::Registration)?;

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
            || verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home),
            || match prior_definition.as_ref() {
                Some(prior) if prior != &rendered.bytes => replace_owned_bytes(
                    &paths.unit_file,
                    prior,
                    &rendered.bytes,
                    target.service_generation(),
                ),
                _ => publish_bytes(
                    &paths.unit_file,
                    &rendered.bytes,
                    0o600,
                    target.service_generation(),
                ),
            },
            || bytes_match(&paths.unit_file, &rendered.bytes),
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
            || verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home),
            || manager.reload(),
            || Ok(true),
        )?;
        planned_physical(
            progress,
            "create-systemd-wants-directory",
            LifecycleEffect::CreateSystemdWantsDirectory {
                target: target.clone(),
            },
            || verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home),
            || create_owned_directory_chain(&paths.wants_directory),
            || owned_directory_present(&paths.wants_directory),
        )?;
        planned_physical(
            progress,
            "publish-systemd-wants-link",
            LifecycleEffect::PublishSystemdWantsLink {
                target: target.clone(),
            },
            || verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home),
            || {
                publish_wants_link(
                    &paths.wants_directory,
                    &paths.wants_link,
                    &paths.unit_file,
                    target.service_generation(),
                )
            },
            || wants_link_matches(&paths.wants_link, &paths.unit_file),
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
        start_unit(
            progress,
            &unit,
            &target,
            &data,
            LinuxRuntime {
                manager: manager.as_ref(),
                context: self.runtime_context.as_ref(),
                clock: self.clock.as_ref(),
            },
        )?;
        progress.history_observed(ReconciliationHistoryFact::BootstrapCommandCompleted);
        progress.history_observed(ReconciliationHistoryFact::BootstrapCommandSucceeded);

        let ready = wait_ready(
            &paths,
            &unit,
            &target,
            &rendered,
            &data,
            LinuxRuntime {
                manager: manager.as_ref(),
                context: self.runtime_context.as_ref(),
                clock: self.clock.as_ref(),
            },
        )?;
        let incarnation = ready.audit_identity()?;
        let native = exact_native(
            manager.as_ref(),
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
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid {
            return Err(GatewayError::Registration(
                "The packaged gateway refuses a set-user-ID process".into(),
            ));
        }
        let manager = self
            .manager_factory
            .connect(effective_uid)
            .map_err(GatewayError::Registration)?;
        let paths = self.paths(&unit)?;
        verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home)
            .map_err(GatewayError::Registration)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        if recovery.pending_step().is_some_and(|step| {
            matches!(
                step.step().effect(),
                LifecycleEffect::PublishServiceDefinition { target }
                    if target == recovery.target()
            )
        }) {
            settle_recovered_definition(
                &paths,
                recovery.target(),
                DefinitionAuthority {
                    configuration: &self.configuration,
                    data: &data,
                    home: &self.home,
                },
            )
            .map_err(GatewayError::Registration)?;
        }
        let advertisement = self
            .runtime_context
            .discover_endpoint(&data)
            .map_err(GatewayError::Registration)?;
        let snapshot = manager
            .snapshot(&unit)
            .map_err(GatewayError::Registration)?;
        let observed = portable_incarnation(&unit, snapshot.as_ref(), advertisement.as_ref())?;
        if snapshot.as_ref().is_some_and(|state| {
            state.main_process_id != 0
                || !matches!(state.active_state.as_str(), "inactive" | "failed")
        }) && observed.is_none()
        {
            return Err(GatewayError::Registration(
                "Fresh recovery found an active or ambiguous systemd process without exact endpoint identity"
                    .into(),
            ));
        }
        let broad_artifact_present = exact_target_artifact_present(
            &paths,
            &unit,
            recovery.target(),
            snapshot.as_ref(),
            DefinitionAuthority {
                configuration: &self.configuration,
                data: &data,
                home: &self.home,
            },
        )
        .map_err(GatewayError::Registration)?;
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
            let artifact_present = recovery_artifact_present(
                step.step().effect(),
                &paths,
                recovery.target(),
                snapshot.as_ref(),
                observed.as_ref(),
                DefinitionAuthority {
                    configuration: &self.configuration,
                    data: &data,
                    home: &self.home,
                },
            )
            .map_err(GatewayError::Registration)?;
            let observation = LifecycleObservation::new(
                recovery
                    .latest_observation()
                    .map_or(1, |value| value.version().saturating_add(1)),
                observed,
                artifact_present,
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
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid {
            return Err(GatewayError::Stop(
                "The packaged gateway refuses a set-user-ID process".into(),
            ));
        }
        let manager = self
            .manager_factory
            .connect(effective_uid)
            .map_err(GatewayError::Stop)?;
        let paths = self
            .paths(&unit)
            .map_err(|error| GatewayError::Stop(error.to_string()))?;
        verify_systemd_authority(manager.as_ref(), &paths, &unit, &self.home)
            .map_err(GatewayError::Stop)?;
        let target = intended.audit_identity()?.target().clone();
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        let advertisement = self
            .runtime_context
            .discover_endpoint(&data)
            .map_err(GatewayError::Stop)?;
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
        let observation = LifecycleObservation::new(
            2,
            observed,
            wants_link_matches(&paths.wants_link, &paths.unit_file).map_err(GatewayError::Stop)?,
        );
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
        runtime_artifact_present(destination, fingerprint).map_err(GatewayError::Registration)?,
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
        staging_runtime_present(root, &staging_generation).map_err(GatewayError::Registration)?,
    )?;
    cleanup_result.map_err(GatewayError::Registration)?;
    result.map_err(GatewayError::Registration)
}

fn planned_physical(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    authorize: impl FnOnce() -> Result<(), String>,
    run: impl FnOnce() -> Result<(), String>,
    present: impl FnOnce() -> Result<bool, String>,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &step, &[])?;
    let result = authorize().and_then(|()| run());
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
        present().map_err(GatewayError::Registration)?,
    )?;
    result.map_err(GatewayError::Registration)
}

fn recovery_artifact_present(
    effect: &LifecycleEffect,
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    snapshot: Option<&UnitSnapshot>,
    observed: Option<&ReconciliationIncarnation>,
    authority: DefinitionAuthority<'_>,
) -> Result<bool, String> {
    match effect {
        LifecycleEffect::StageRuntime { fingerprint }
        | LifecycleEffect::PruneRuntime { fingerprint } => {
            let runtime = paths.runtime_root.join(fingerprint);
            runtime_artifact_present(&runtime, fingerprint)
        }
        LifecycleEffect::RemoveStagingRuntime { generation } => {
            staging_runtime_present(&paths.runtime_root, generation)
        }
        LifecycleEffect::PublishServiceDefinition { target: planned } => {
            if planned != target {
                return Err("Recovered definition plan targets another lifecycle".into());
            }
            definition_matches_target(paths, target, authority)
        }
        LifecycleEffect::CreateSystemdWantsDirectory { .. } => {
            owned_directory_present(&paths.wants_directory)
        }
        LifecycleEffect::PublishSystemdWantsLink { .. } => {
            wants_link_matches(&paths.wants_link, &paths.unit_file)
        }
        LifecycleEffect::ReloadSystemdManager { .. } => {
            Ok(snapshot.is_some_and(|state| snapshot_declares_target(state, target)))
        }
        LifecycleEffect::StartSystemdUnit { .. }
        | LifecycleEffect::BootstrapService { .. }
        | LifecycleEffect::AdoptReadyIncarnation { .. } => {
            Ok(observed.is_some_and(|incarnation| incarnation.target() == target))
        }
        LifecycleEffect::StopSystemdUnit { .. } | LifecycleEffect::UnloadService { .. } => {
            Ok(snapshot.is_some())
        }
        LifecycleEffect::RequestRetirement { .. } => {
            Err("A non-systemd retirement effect cannot be recovered by the Linux host".into())
        }
        LifecycleEffect::RequestSystemdRetirement {
            incarnation,
            request_id,
        } => {
            if incarnation.target().service() != target.service() {
                return Err("Recovered retirement targets another systemd service".into());
            }
            retirement_artifact_present(authority.data, request_id, incarnation, target, observed)
        }
        LifecycleEffect::StopAgents { incarnation } => Ok(observed == Some(incarnation)),
    }
}

fn runtime_artifact_present(path: &Path, fingerprint: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_runtime(path, fingerprint).map(|()| true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn owned_directory_present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            verify_owned_path_if_present(path, true)?;
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn exact_target_artifact_present(
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    snapshot: Option<&UnitSnapshot>,
    authority: DefinitionAuthority<'_>,
) -> Result<bool, String> {
    let runtime = paths.runtime_root.join(target.runtime_fingerprint());
    let runtime_present = match fs::symlink_metadata(&runtime) {
        Ok(_) => {
            validate_runtime(&runtime, target.runtime_fingerprint())?;
            true
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    let definition_present = definition_matches_target(paths, target, authority)?;
    if definition_present
        && snapshot.is_some_and(|state| {
            state.id != unit.as_str() || !snapshot_declares_target(state, target)
        })
    {
        return Err("The manager snapshot contradicts the restored gateway definition".into());
    }
    let link_present = wants_link_matches(&paths.wants_link, &paths.unit_file)?;
    Ok(runtime_present || definition_present || link_present)
}

fn definition_matches_target(
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    authority: DefinitionAuthority<'_>,
) -> Result<bool, String> {
    let Some(bytes) = owned_file_bytes(&paths.unit_file)? else {
        return Ok(false);
    };
    if !definition_bytes_match_target(&bytes, paths, target, authority)? {
        return Err("The restored gateway definition does not exactly match its target".into());
    }
    Ok(true)
}

fn definition_bytes_match_target(
    bytes: &[u8],
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    authority: DefinitionAuthority<'_>,
) -> Result<bool, String> {
    let agent_path = rendered_agent_path(bytes)?;
    let unit =
        SystemdUnitName::parse(target.service().to_owned()).map_err(|error| error.to_string())?;
    let expected = render(UnitDefinition {
        unit: &unit,
        runtime: &paths.runtime_root.join(target.runtime_fingerprint()),
        configuration: authority.configuration,
        data: authority.data,
        home: authority.home,
        agent_path: &agent_path,
        fingerprint: target.runtime_fingerprint(),
        generation: target.service_generation(),
    })?;
    Ok(bytes == expected.bytes)
}

fn settle_recovered_definition(
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    authority: DefinitionAuthority<'_>,
) -> Result<(), String> {
    let transaction = definition_transaction(&paths.unit_file, target.service_generation())?;
    if transaction.publish_temporary.is_none() && transaction.replace_temporary.is_none() {
        return Ok(());
    }
    let candidates = [
        transaction.current.as_deref(),
        transaction.publish_temporary.as_deref(),
        transaction.replace_temporary.as_deref(),
    ];
    let mut expected = None;
    for candidate in candidates.into_iter().flatten() {
        if definition_bytes_match_target(
            candidate,
            paths,
            target,
            DefinitionAuthority {
                configuration: authority.configuration,
                data: authority.data,
                home: authority.home,
            },
        )? {
            if expected
                .as_ref()
                .is_some_and(|known: &&[u8]| *known != candidate)
            {
                return Err(
                    "Definition transaction contains multiple target representations".into(),
                );
            }
            expected = Some(candidate);
        }
    }
    let expected = expected.ok_or_else(|| {
        "Definition transaction has no exact representation of its journal target".to_string()
    })?;
    settle_definition_transaction(&paths.unit_file, expected, target.service_generation())?;
    Ok(())
}

fn snapshot_declares_target(snapshot: &UnitSnapshot, target: &ReconciliationTarget) -> bool {
    snapshot.exec_start_ex.first().is_some_and(|entry| {
        environment_value(&entry.1, "NESSA_RUNTIME_FINGERPRINT")
            == Some(target.runtime_fingerprint())
            && environment_value(&entry.1, "NESSA_SERVICE_GENERATION")
                == Some(target.service_generation())
    })
}

fn start_unit(
    progress: &dyn GatewayReconciliationProgress,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    data: &Path,
    runtime: LinuxRuntime<'_>,
) -> Result<(), GatewayError> {
    run_systemd_job(
        progress,
        "start-systemd-unit",
        LifecycleEffect::StartSystemdUnit {
            manager: runtime.manager.identity().clone(),
            unit: unit.clone(),
            mode: SystemdJobMode::Fail,
        },
        runtime.manager,
        unit,
        target,
        SystemdJobOperation::Start,
        data,
        runtime.context,
        runtime.clock,
    )
}

fn stop_unit(
    progress: &dyn GatewayReconciliationProgress,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    data: &Path,
    runtime: LinuxRuntime<'_>,
) -> Result<(), GatewayError> {
    run_systemd_job(
        progress,
        "stop-systemd-unit",
        LifecycleEffect::StopSystemdUnit {
            manager: runtime.manager.identity().clone(),
            unit: unit.clone(),
            mode: SystemdJobMode::Fail,
        },
        runtime.manager,
        unit,
        target,
        SystemdJobOperation::Stop,
        data,
        runtime.context,
        runtime.clock,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_systemd_job(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    manager: &dyn LinuxUserManager,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    operation: SystemdJobOperation,
    data: &Path,
    runtime_context: &dyn LinuxRuntimeContext,
    clock: &dyn MonotonicClock,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &step, &[])?;
    let handle = match manager
        .recheck_identity()
        .and_then(|()| manager.enqueue(operation, unit, clock))
    {
        Ok(handle) => handle,
        Err(error) => {
            let (incarnation, snapshot) = observe_after_job(manager, unit, data, runtime_context)?;
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
                incarnation,
                snapshot.is_some(),
            )?;
            return Err(GatewayError::Registration(error));
        }
    };
    progress.native_attempt_recorded(plan_id, step.id(), &handle.attempt)?;
    let terminal = handle.wait(clock, clock.now() + JOB_TIMEOUT);
    let (incarnation, snapshot) = observe_after_job(manager, unit, data, runtime_context)?;
    let completion = classify_terminal(manager, unit, &terminal);
    progress.effect_completed(plan_id, step.id(), &completion)?;
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

fn observe_after_job(
    manager: &dyn LinuxUserManager,
    unit: &SystemdUnitName,
    data: &Path,
    runtime_context: &dyn LinuxRuntimeContext,
) -> Result<(Option<ReconciliationIncarnation>, Option<UnitSnapshot>), GatewayError> {
    let snapshot = manager.snapshot(unit).map_err(GatewayError::Registration)?;
    let advertisement = runtime_context
        .discover_endpoint(data)
        .map_err(GatewayError::Registration)?;
    let incarnation = portable_incarnation(unit, snapshot.as_ref(), advertisement.as_ref())?;
    if snapshot.as_ref().is_some_and(|state| {
        state.main_process_id != 0 || !matches!(state.active_state.as_str(), "inactive" | "failed")
    }) && incarnation.is_none()
    {
        return Err(GatewayError::Registration(
            "Fresh systemd job observation found an active or ambiguous process without exact endpoint identity"
                .into(),
        ));
    }
    Ok((incarnation, snapshot))
}

fn classify_terminal(
    manager: &dyn LinuxUserManager,
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
    manager: &dyn LinuxUserManager,
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
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    rendered: &RenderedUnit,
    data: &Path,
    runtime: LinuxRuntime<'_>,
) -> Result<ReconciledGateway, GatewayError> {
    let deadline = runtime.clock.now() + READY_TIMEOUT;
    loop {
        let advertisement = runtime
            .context
            .discover_endpoint(data)
            .map_err(GatewayError::Registration)?;
        if let Some(ready) = exact_ready(
            runtime.manager,
            paths,
            unit,
            target,
            rendered,
            advertisement.as_ref(),
        )? {
            return Ok(ready);
        }
        if runtime.clock.now() >= deadline {
            return Err(GatewayError::Registration(
                "The systemd gateway did not publish matching readiness before its deadline".into(),
            ));
        }
        runtime.clock.wait(Duration::from_millis(100));
    }
}

fn snapshot_matches(
    snapshot: &UnitSnapshot,
    manager: &dyn LinuxUserManager,
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
        && fs::canonicalize(&paths.unit_file).is_ok_and(|canonical| canonical == paths.unit_file)
        && snapshot.drop_in_paths.is_empty()
        && snapshot.active_state == "active"
        && snapshot.sub_state == "running"
        && snapshot.invocation.is_some()
        && snapshot.main_process_id != 0
        && snapshot.service_type == "simple"
        && snapshot.restart == "on-failure"
        && snapshot.restart_microseconds == 5_000_000
        && snapshot.timeout_stop_microseconds == 30_000_000
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
    manager: &dyn LinuxUserManager,
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
        || snapshot.service_type != "simple"
        || snapshot.restart != "on-failure"
        || snapshot.restart_microseconds != 5_000_000
        || snapshot.timeout_stop_microseconds != 30_000_000
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
    let stem = unit
        .as_str()
        .strip_suffix(".service")
        .ok_or_else(|| "The gateway unit is not a service".to_string())?;
    let mut drop_ins = vec![format!("{}.d", unit.as_str()), "service.d".into()];
    drop_ins.extend(
        stem.match_indices('-')
            .map(|(index, _)| format!("{}.service.d", &stem[..=index])),
    );
    for directory in unit_path {
        match fs::read_dir(directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| error.to_string())?;
                    let metadata =
                        fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
                    if entry.file_name() != std::ffi::OsStr::new(unit.as_str())
                        && metadata.file_type().is_symlink()
                        && fs::read_link(entry.path())
                            .ok()
                            .and_then(|target| target.file_name().map(ToOwned::to_owned))
                            .as_deref()
                            == Some(std::ffi::OsStr::new(unit.as_str()))
                    {
                        return Err("A systemd unit alias targets the gateway unit".into());
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        for drop_in in &drop_ins {
            if fs::symlink_metadata(Path::new(directory).join(drop_in)).is_ok() {
                return Err(format!(
                    "A systemd drop-in directory can alter the gateway unit: {drop_in}"
                ));
            }
        }
    }
    verify_owned_path_if_present(&paths.unit_root, true)?;
    verify_owned_path_if_present(&paths.wants_directory, true)?;
    if let Ok(metadata) = fs::symlink_metadata(&paths.unit_file) {
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            return Err("The installed gateway unit has unsafe ownership, links, or mode".into());
        }
    }
    if fs::symlink_metadata(&paths.wants_link).is_ok()
        && !wants_link_matches(&paths.wants_link, &paths.unit_file)?
    {
        return Err("A conflicting persistent gateway link already exists".into());
    }
    Ok(())
}

fn verify_systemd_authority(
    manager: &dyn LinuxUserManager,
    paths: &LinuxGatewayPaths,
    unit: &SystemdUnitName,
    home: &Path,
) -> Result<(), String> {
    manager.recheck_identity()?;
    verify_unit_authority(&manager.unit_path()?, paths, unit)?;
    verify_manager_environment(&manager.environment()?, paths, home)
}

fn verify_owned_path_if_present(path: &Path, private: bool) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || (private && metadata.permissions().mode() & 0o077 != 0)
    {
        return Err(format!(
            "The gateway directory has unsafe ownership, type, or mode: {}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_manager_environment(
    environment: &[String],
    paths: &LinuxGatewayPaths,
    home: &Path,
) -> Result<(), String> {
    for (name, expected) in [
        ("HOME", home),
        ("XDG_CONFIG_HOME", paths.config_root.as_path()),
        ("XDG_DATA_HOME", paths.data_root.as_path()),
    ] {
        if let Some(observed) = environment_value(environment, name) {
            if Path::new(observed) != expected {
                return Err(format!(
                    "The systemd user manager {name} differs from the desktop authority"
                ));
            }
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
    unit: &SystemdUnitName,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    data: &Path,
    runtime: LinuxRuntime<'_>,
) -> Result<(), GatewayError> {
    let request_id = random_uuid().map_err(GatewayError::Registration)?;
    let step = LifecyclePlanStep::new(
        "primary".into(),
        LifecycleEffect::RequestSystemdRetirement {
            incarnation: prior.clone(),
            request_id: request_id.clone(),
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
    let snapshot = runtime
        .manager
        .snapshot(unit)
        .map_err(GatewayError::Registration)?
        .ok_or_else(|| GatewayError::Registration("The retiring systemd unit vanished".into()))?;
    let advertisement = runtime
        .context
        .discover_endpoint(data)
        .map_err(GatewayError::Registration)?;
    if portable_incarnation(unit, Some(&snapshot), advertisement.as_ref())?.as_ref() != Some(prior)
        || runtime
            .manager
            .get_unit_by_pid(prior.process_id())
            .map_err(GatewayError::Registration)?
            != snapshot.object_path
    {
        return Err(GatewayError::Registration(
            "The retiring systemd process changed after the effect plan was acknowledged".into(),
        ));
    }
    let result = retire(data, &request_id, prior, target, &descriptor, runtime.clock);
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    progress.effect_completed("request-systemd-retirement", step.id(), &completion)?;
    let snapshot = runtime
        .manager
        .snapshot(unit)
        .map_err(GatewayError::Registration)?;
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
struct RetirementRequestFile {
    request_id: String,
    target_fingerprint: String,
    running_instance: String,
    running_generation: String,
    target_generation: String,
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

fn retirement_artifact_present(
    data: &Path,
    request_id: &str,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    observed: Option<&ReconciliationIncarnation>,
) -> Result<bool, String> {
    let request_path = data.join("gateway-upgrade/request.json");
    let file = match nessa_local_storage::open(
        &request_path,
        nessa_local_storage::OpenMode::ReadNonblocking,
    ) {
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
        return Err("Gateway retirement request exceeds its limit".into());
    }
    let request: RetirementRequestFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid gateway retirement request: {error}"))?;
    let agrees = request.request_id == request_id
        && request.target_fingerprint == target.runtime_fingerprint()
        && request.running_instance == prior.runtime_instance()
        && request.running_generation == prior.target().service_generation()
        && request.target_generation == target.service_generation();
    if !agrees {
        return Err("Gateway retirement request disagrees with its journal plan".into());
    }
    if observed != Some(prior) {
        return Err("Gateway retirement recovery lost the planned running incarnation".into());
    }
    retirement_acknowledged(
        &data.join("gateway-upgrade/result.json"),
        request_id,
        prior,
        target,
    )
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
    request_id: &str,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    descriptor: &OwnedFd,
    clock: &dyn MonotonicClock,
) -> Result<(), String> {
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory_beneath(data, Path::new("gateway-upgrade"))
        .map_err(|error| error.to_string())?;
    let request = serde_json::to_vec(&serde_json::json!({
        "requestId": request_id,
        "targetFingerprint": target.runtime_fingerprint(),
        "runningInstance": prior.runtime_instance(),
        "runningGeneration": prior.target().service_generation(),
        "targetGeneration": target.service_generation(),
    }))
    .map_err(|error| error.to_string())?;
    atomic_write(&directory, Path::new("request.json"), &request)?;
    let sent = retirement_pidfd_signal(descriptor.as_raw_fd());
    if sent != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let deadline = clock.now() + Duration::from_secs(75);
    while clock.now() < deadline {
        if retirement_acknowledged(&directory.join("result.json"), request_id, prior, target)? {
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

fn atomic_write(directory: &Path, destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temporary = nessa_local_storage::PrivateTempFile::new_beneath(directory, Path::new(""))
        .map_err(|error| error.to_string())?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist_beneath(destination)
        .map_err(|error| error.to_string())?;
    nessa_local_storage::sync_directory(directory).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        application::{
            testing::discard_reconciliation_audit, GatewayReconciliationAudit,
            GatewayReconciliationRequest, SystemMonotonicClock,
        },
        domain::value_objects::{
            ReconciliationCorrelation, ReconciliationEvidence, ReconciliationInitiator,
            SystemdInvocationId, SystemdManagerIdentity,
        },
    };
    use std::os::unix::fs::PermissionsExt;

    #[derive(Clone)]
    struct FixedManagerFactory {
        identity: SystemdManagerIdentity,
        unit_path: Vec<String>,
        snapshot: Option<UnitSnapshot>,
        error: Option<String>,
    }

    impl LinuxManagerFactory for FixedManagerFactory {
        fn connect(&self, _: u32) -> Result<Box<dyn LinuxUserManager>, String> {
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(Box::new(FixedManager {
                identity: self.identity.clone(),
                unit_path: self.unit_path.clone(),
                snapshot: self.snapshot.clone(),
            }))
        }
    }

    struct FixedManager {
        identity: SystemdManagerIdentity,
        unit_path: Vec<String>,
        snapshot: Option<UnitSnapshot>,
    }

    impl LinuxUserManager for FixedManager {
        fn identity(&self) -> &SystemdManagerIdentity {
            &self.identity
        }
        fn recheck_identity(&self) -> Result<(), String> {
            Ok(())
        }
        fn unit_path(&self) -> Result<Vec<String>, String> {
            Ok(self.unit_path.clone())
        }
        fn environment(&self) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn reload(&self) -> Result<(), String> {
            unreachable!("recovery must not replay manager effects")
        }
        fn unit_file_state(&self, _: &SystemdUnitName) -> Result<String, String> {
            unreachable!("recovery must not query enablement outside its observation")
        }
        fn get_unit_by_pid(&self, _: u32) -> Result<String, String> {
            unreachable!("recovery must not infer identity by PID")
        }
        fn snapshot(&self, _: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String> {
            Ok(self.snapshot.clone())
        }
        fn enqueue(
            &self,
            _: SystemdJobOperation,
            _: &SystemdUnitName,
            _: &dyn MonotonicClock,
        ) -> Result<super::super::user_manager::JobHandle, String> {
            unreachable!("recovery must not enqueue systemd work")
        }
    }

    struct FixedRuntimeContext {
        effective_uid: u32,
        real_uid: u32,
        endpoint_error: Option<String>,
    }

    impl LinuxRuntimeContext for FixedRuntimeContext {
        fn user_ids(&self) -> (u32, u32) {
            (self.effective_uid, self.real_uid)
        }

        fn discover_endpoint(
            &self,
            _: &Path,
        ) -> Result<Option<GatewayEndpointAdvertisement>, String> {
            self.endpoint_error
                .as_ref()
                .map_or(Ok(None), |error| Err(error.clone()))
        }
    }

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
            service_type: "simple".into(),
            restart: "on-failure".into(),
            restart_microseconds: 5_000_000,
            timeout_stop_microseconds: 30_000_000,
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
    fn recovery_controller_uses_injected_uid_manager_and_endpoint_boundaries() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let config = root.join("config");
        let data = root.join("data");
        let configuration =
            ServiceConfiguration::new("prod".into(), data.join("nessa"), None, 7420, None).unwrap();
        let (unit, target, active) = fixture();
        let unit_path = vec![config.join("systemd/user").to_string_lossy().into_owned()];
        let correlation = |serial| {
            ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{serial:012x}"))
                .unwrap()
        };
        let evidence = ReconciliationEvidence::new(
            ReconciliationCause::Startup,
            ReconciliationInitiator::DesktopHost,
        )
        .unwrap();
        let request = GatewayReconciliationRequest::new(correlation(1), evidence);
        let attempt = GatewayReconciliationAttempt::new(correlation(2), request).unwrap();
        let recovery =
            GatewayLifecycleRecovery::new(attempt.clone(), target, None, false, None, None, None);
        let journal = discard_reconciliation_audit().open(&attempt, None).unwrap();
        let identity = active.manager.clone();
        let effective_uid = unsafe { libc::geteuid() };

        let cases = [
            (effective_uid + 1, effective_uid, None, None, false),
            (
                effective_uid,
                effective_uid,
                Some("manager unavailable"),
                None,
                false,
            ),
            (
                effective_uid,
                effective_uid,
                None,
                Some("endpoint unavailable"),
                false,
            ),
            (effective_uid, effective_uid, None, None, true),
        ];
        for (effective, real, manager_error, endpoint_error, active_snapshot) in cases {
            let gateway = SystemdGateway {
                configuration: configuration.clone(),
                home: home.clone(),
                config_home: Some(config.clone().into_os_string()),
                data_home: Some(data.clone().into_os_string()),
                clock: Arc::new(SystemMonotonicClock),
                manager_factory: Arc::new(FixedManagerFactory {
                    identity: identity.clone(),
                    unit_path: unit_path.clone(),
                    snapshot: active_snapshot.then(|| active.clone()),
                    error: manager_error.map(str::to_owned),
                }),
                runtime_context: Arc::new(FixedRuntimeContext {
                    effective_uid: effective,
                    real_uid: real,
                    endpoint_error: endpoint_error.map(str::to_owned),
                }),
            };
            assert!(gateway.recover(&recovery, journal.as_ref()).is_err());
        }

        let gateway = SystemdGateway {
            configuration,
            home,
            config_home: Some(config.into_os_string()),
            data_home: Some(data.into_os_string()),
            clock: Arc::new(SystemMonotonicClock),
            manager_factory: Arc::new(FixedManagerFactory {
                identity,
                unit_path,
                snapshot: None,
                error: None,
            }),
            runtime_context: Arc::new(FixedRuntimeContext {
                effective_uid,
                real_uid: effective_uid,
                endpoint_error: None,
            }),
        };
        assert!(gateway.recover(&recovery, journal.as_ref()).is_ok());
        assert_eq!(unit.as_str(), recovery.target().service());
    }

    #[test]
    fn retirement_recovery_requires_the_planned_request_and_result_tuple() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().canonicalize().unwrap().join("data");
        nessa_local_storage::create_directory(&data).unwrap();
        nessa_local_storage::create_directory_beneath(&data, Path::new("gateway-upgrade")).unwrap();
        let directory = data.join("gateway-upgrade");
        let request_id = "550e8400-e29b-41d4-a716-446655440000";
        let (unit, old_target, _) = fixture();
        let prior = ReconciliationIncarnation::new(
            old_target,
            "550e8400-e29b-41d4-a716-446655440001".into(),
            99,
            7420,
        )
        .unwrap();
        let target =
            ReconciliationTarget::new(unit.as_str().into(), "c".repeat(64), "d".repeat(64))
                .unwrap();
        let request = serde_json::to_vec(&serde_json::json!({
            "requestId": request_id,
            "targetFingerprint": target.runtime_fingerprint(),
            "runningInstance": prior.runtime_instance(),
            "runningGeneration": prior.target().service_generation(),
            "targetGeneration": target.service_generation(),
        }))
        .unwrap();
        atomic_write(&directory, Path::new("request.json"), &request).unwrap();
        let result = serde_json::to_vec(&serde_json::json!({
            "requestId": request_id,
            "targetFingerprint": target.runtime_fingerprint(),
            "runningFingerprint": prior.target().runtime_fingerprint(),
            "runningInstance": prior.runtime_instance(),
            "requestedInstance": prior.runtime_instance(),
            "runningGeneration": prior.target().service_generation(),
            "requestedRunningGeneration": prior.target().service_generation(),
            "targetGeneration": target.service_generation(),
            "retired": true,
            "retirementRequestId": request_id,
            "retirementCause": {
                "principalId": "gateway",
                "surfaceId": "gateway_upgrade",
                "requestId": request_id,
            },
            "cleanupError": null,
            "auditError": null,
        }))
        .unwrap();
        atomic_write(&directory, Path::new("result.json"), &result).unwrap();

        assert!(
            retirement_artifact_present(&data, request_id, &prior, &target, Some(&prior),).unwrap()
        );
        assert!(retirement_artifact_present(
            &data,
            "550e8400-e29b-41d4-a716-446655440002",
            &prior,
            &target,
            Some(&prior),
        )
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_manager_factory_refuses_an_account_without_linger_authority() {
        assert!(NativeLinuxManagerFactory.connect(u32::MAX).is_err());
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
        let mut wrong = snapshot.clone();
        wrong.service_type = "forking".into();
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.restart = "always".into();
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.restart_microseconds = 1;
        cases.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.timeout_stop_microseconds = 1;
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
        fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o700)).unwrap();
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let paths = LinuxGatewayPaths {
            config_root: temporary.path().join("config"),
            data_root: temporary.path().join("data"),
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

    #[test]
    fn authority_refuses_every_systemd_drop_in_scope_and_manager_path_disagreement() {
        let temporary = tempfile::tempdir().unwrap();
        let owned = temporary.path().join("owned");
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o700)).unwrap();
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let paths = LinuxGatewayPaths {
            config_root: temporary.path().join("config"),
            data_root: temporary.path().join("data"),
            unit_file: owned.join(unit.as_str()),
            wants_directory: owned.join("default.target.wants"),
            wants_link: owned.join("default.target.wants").join(unit.as_str()),
            runtime_root: temporary.path().join("runtime"),
            unit_root: owned.clone(),
        };
        let search = vec![owned.to_string_lossy().into_owned()];
        for drop_in in [
            "nessa-gateway-prod.service.d",
            "nessa-gateway-.service.d",
            "nessa-.service.d",
            "service.d",
        ] {
            let path = owned.join(drop_in);
            fs::create_dir(&path).unwrap();
            assert!(verify_unit_authority(&search, &paths, &unit).is_err());
            fs::remove_dir(path).unwrap();
        }
        assert!(verify_manager_environment(
            &["XDG_CONFIG_HOME=/elsewhere".into()],
            &paths,
            temporary.path()
        )
        .is_err());
    }
}
