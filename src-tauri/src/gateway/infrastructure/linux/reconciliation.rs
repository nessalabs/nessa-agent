//! Reconciles one packaged gateway with the account's systemd user manager.

use super::super::startup_failure::{recorded_failure, RecordedFailure};
use super::{
    paths::LinuxGatewayPaths,
    process::{
        verify_pidfd_support, LinuxProcess, LinuxProcessFactory, LinuxProcessSignalError,
        LinuxSignalAuthority, NativeLinuxProcessFactory,
    },
    staging::{
        bytes_match, create_owned_directory_transaction, definition_transaction,
        discard_wants_link_temporary, owned_directory_transaction_present, owned_file_bytes,
        publish_bytes, publish_runtime, publish_wants_link, remove_staging_runtime,
        replace_owned_bytes, runtime_fingerprint, settle_definition_transaction,
        settle_owned_directory_transaction, settle_recovered_owned_directory_transaction,
        settle_wants_link_transaction, staging_runtime_present, validate_runtime,
        wants_link_matches, wants_link_temporary_present,
    },
    unit::{render, rendered_agent_path, unit_name, RenderedUnit, UnitDefinition},
    user_manager::{verify_linger, JobTerminal, UnitSnapshot, UserManager},
};
use crate::gateway::{
    application::{
        GatewayError, GatewayHost, GatewayLifecycleRecovery, GatewayPhysicalResult,
        GatewayReconciliationAttempt, GatewayReconciliationIntent,
        GatewayReconciliationJournalSession, GatewayReconciliationProgress, GatewayStopSession,
        MonotonicClock, ReconciledGateway, ReconciliationHistoryFact,
    },
    domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, LifecycleEffect, LifecycleEffectPredicate,
        LifecycleFailedPhase, LifecycleObservation, LifecycleObservationSource,
        LifecyclePhysicalOutcome, LifecyclePlanStep, ReconciliationCause,
        ReconciliationCleanupDecision, ReconciliationIncarnation, ReconciliationTarget, SearchPath,
        ServiceConfiguration, StartupFailureRecoveryAuthority, SystemdJobAttempt,
        SystemdJobConclusion, SystemdJobMode, SystemdJobOperation, SystemdJobTerminal,
        SystemdManagerIdentity, SystemdRuntimeObservation, SystemdUnitName, SystemdUnitState,
    },
};
use nessa_gateway_endpoint::{
    domain::GatewayEndpointAdvertisement, infrastructure::FileEndpointDiscovery,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const JOB_TIMEOUT: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(45);

pub(crate) struct SystemdGateway {
    configuration: ServiceConfiguration,
    home: PathBuf,
    clock: Arc<dyn MonotonicClock>,
    manager_factory: Arc<dyn LinuxManagerFactory>,
    runtime_context: Arc<dyn LinuxRuntimeContext>,
    process_factory: Arc<dyn LinuxProcessFactory>,
}

trait LinuxManagerFactory: Send + Sync {
    fn verify_prerequisites(&self, expected_uid: u32) -> Result<(), String>;
    fn connect(&self, expected_uid: u32) -> Result<Box<dyn LinuxUserManager>, String>;
}

trait LinuxRuntimeContext: Send + Sync {
    fn user_ids(&self) -> (u32, u32);
    fn xdg_paths(&self) -> (Option<OsString>, Option<OsString>, Option<OsString>);
    fn observe_endpoint_health(&self, data: &Path) -> Result<Option<LinuxEndpointHealth>, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LinuxEndpointHealth {
    advertisement: GatewayEndpointAdvertisement,
}

impl LinuxEndpointHealth {
    fn advertisement(&self) -> &GatewayEndpointAdvertisement {
        &self.advertisement
    }
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

struct RetirementAuthority<'a> {
    configuration: &'a ServiceConfiguration,
    data: &'a Path,
    home: &'a Path,
    paths: &'a LinuxGatewayPaths,
    process_factory: &'a dyn LinuxProcessFactory,
}

struct PreflightEvidence<'a> {
    runtime: &'a Path,
    fingerprint: &'a str,
    unit: &'a SystemdUnitName,
    paths: &'a LinuxGatewayPaths,
    installed: Option<&'a UnitSnapshot>,
    data: &'a Path,
    advertisement: Option<&'a GatewayEndpointAdvertisement>,
    startup_failure: Option<&'a StartupFailureRecoveryAuthority>,
}

struct SystemdJobAuthority<'a> {
    manager: &'a dyn LinuxUserManager,
    unit: &'a SystemdUnitName,
    target: &'a ReconciliationTarget,
    operation: SystemdJobOperation,
    data: &'a Path,
    paths: &'a LinuxGatewayPaths,
    runtime_context: &'a dyn LinuxRuntimeContext,
    clock: &'a dyn MonotonicClock,
}

struct RecoveryPhysical<'a> {
    manager: &'a SystemdManagerIdentity,
    snapshot: Option<&'a UnitSnapshot>,
    incarnation: Option<&'a ReconciliationIncarnation>,
    target_artifact_present: bool,
}

struct ObservedSystemdState {
    snapshot: Option<UnitSnapshot>,
    incarnation: Option<ReconciliationIncarnation>,
    native: Option<SystemdRuntimeObservation>,
    unit_state: SystemdUnitState,
}

impl ObservedSystemdState {
    fn lifecycle_observation(
        &self,
        version: u64,
        target_artifact_present: bool,
    ) -> Result<LifecycleObservation, GatewayError> {
        match (&self.incarnation, &self.native) {
            (Some(incarnation), Some(native)) => LifecycleObservation::with_systemd(
                version,
                incarnation.clone(),
                target_artifact_present,
                native.clone(),
            )
            .map_err(|error| GatewayError::Registration(error.to_string())),
            (None, None) => LifecycleObservation::with_systemd_state(
                version,
                target_artifact_present,
                self.unit_state,
            )
            .map_err(|error| GatewayError::Registration(error.to_string())),
            _ => Err(GatewayError::Registration(
                "Systemd observation split portable and native runtime evidence".into(),
            )),
        }
    }

    fn record(
        self,
        progress: &dyn GatewayReconciliationProgress,
        source: &LifecycleObservationSource,
        target_artifact_present: bool,
    ) -> Result<LifecycleObservation, GatewayError> {
        match (self.incarnation, self.native) {
            (Some(incarnation), Some(native)) => {
                progress.systemd_observed(source, incarnation, target_artifact_present, native)
            }
            (None, None) => {
                progress.systemd_state_observed(source, target_artifact_present, self.unit_state)
            }
            _ => Err(GatewayError::Registration(
                "Systemd observation split portable and native runtime evidence".into(),
            )),
        }
    }
}

trait LinuxUserManager: Send + Sync {
    fn identity(&self) -> &crate::gateway::domain::value_objects::SystemdManagerIdentity;
    fn recheck_identity(&self) -> Result<(), String>;
    fn unit_path(&self) -> Result<Vec<String>, String>;
    fn reload(&self) -> Result<(), String>;
    fn unit_file_state(&self, unit: &SystemdUnitName) -> Result<String, String>;
    fn get_unit_by_pid(&self, process_id: u32) -> Result<String, String>;
    fn snapshot(&self, unit: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String>;
    fn enqueue(
        &self,
        operation: SystemdJobOperation,
        unit: &SystemdUnitName,
        clock: &dyn MonotonicClock,
    ) -> Result<Box<dyn LinuxSystemdJob>, String>;
}

trait LinuxSystemdJob: Send {
    fn attempt(&self) -> &SystemdJobAttempt;
    fn wait(
        self: Box<Self>,
        clock: &dyn MonotonicClock,
        deadline: std::time::Instant,
    ) -> Result<JobTerminal, String>;
}

impl LinuxSystemdJob for super::user_manager::JobHandle {
    fn attempt(&self) -> &SystemdJobAttempt {
        &self.attempt
    }

    fn wait(
        self: Box<Self>,
        clock: &dyn MonotonicClock,
        deadline: std::time::Instant,
    ) -> Result<JobTerminal, String> {
        (*self).wait(clock, deadline)
    }
}

struct NativeLinuxManagerFactory;
struct NativeLinuxRuntimeContext {
    xdg_paths: (Option<OsString>, Option<OsString>, Option<OsString>),
}

impl LinuxManagerFactory for NativeLinuxManagerFactory {
    fn verify_prerequisites(&self, expected_uid: u32) -> Result<(), String> {
        verify_linger(expected_uid)?;
        verify_pidfd_support()
    }

    fn connect(&self, expected_uid: u32) -> Result<Box<dyn LinuxUserManager>, String> {
        self.verify_prerequisites(expected_uid)?;
        UserManager::connect(expected_uid)
            .map(|manager| Box::new(manager) as Box<dyn LinuxUserManager>)
    }
}

impl LinuxRuntimeContext for NativeLinuxRuntimeContext {
    fn user_ids(&self) -> (u32, u32) {
        unsafe { (libc::geteuid(), libc::getuid()) }
    }

    fn xdg_paths(&self) -> (Option<OsString>, Option<OsString>, Option<OsString>) {
        self.xdg_paths.clone()
    }

    fn observe_endpoint_health(&self, data: &Path) -> Result<Option<LinuxEndpointHealth>, String> {
        discover_endpoint(data).map(|advertisement| {
            advertisement.map(|advertisement| LinuxEndpointHealth { advertisement })
        })
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
    ) -> Result<Box<dyn LinuxSystemdJob>, String> {
        self.enqueue(operation, unit, clock)
            .map(|job| Box::new(job) as Box<dyn LinuxSystemdJob>)
    }
}

impl SystemdGateway {
    pub fn new(
        configuration: ServiceConfiguration,
        home: PathBuf,
        xdg_paths: (Option<OsString>, Option<OsString>, Option<OsString>),
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self {
            configuration,
            home,
            clock,
            manager_factory: Arc::new(NativeLinuxManagerFactory),
            runtime_context: Arc::new(NativeLinuxRuntimeContext { xdg_paths }),
            process_factory: Arc::new(NativeLinuxProcessFactory),
        }
    }

    fn paths(&self, unit: &SystemdUnitName) -> Result<LinuxGatewayPaths, GatewayError> {
        let (config_home, data_home, state_home) = self.runtime_context.xdg_paths();
        LinuxGatewayPaths::new(&self.home, config_home, data_home, state_home, unit)
            .map_err(GatewayError::Registration)
    }

    fn revalidate_preflight(
        &self,
        manager: &dyn LinuxUserManager,
        evidence: PreflightEvidence<'_>,
    ) -> Result<(), GatewayError> {
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid {
            return Err(GatewayError::Registration(
                "The packaged gateway account identity changed during preflight".into(),
            ));
        }
        if self.paths(evidence.unit)? != *evidence.paths
            || runtime_fingerprint(evidence.runtime).map_err(GatewayError::Registration)?
                != evidence.fingerprint
            || manager
                .snapshot(evidence.unit)
                .map_err(GatewayError::Registration)?
                .as_ref()
                != evidence.installed
            || self
                .runtime_context
                .observe_endpoint_health(evidence.data)
                .map_err(GatewayError::Registration)?
                .as_ref()
                .map(LinuxEndpointHealth::advertisement)
                != evidence.advertisement
            || evidence.startup_failure.is_some_and(|authority| {
                recorded_failure(&evidence.data.join("logs"))
                    .and_then(|record| record.authority(authority.target().clone()))
                    .as_ref()
                    != Some(authority)
            })
        {
            return Err(GatewayError::Registration(
                "Gateway source, paths, installed unit, or endpoint changed during preflight"
                    .into(),
            ));
        }
        verify_systemd_authority(manager, evidence.paths, evidence.unit)
            .map_err(GatewayError::Registration)
    }

    fn revalidate_effect_authority(
        &self,
        manager: &dyn LinuxUserManager,
        unit: &SystemdUnitName,
        paths: &LinuxGatewayPaths,
    ) -> Result<(), GatewayError> {
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid || self.paths(unit)? != *paths {
            return Err(GatewayError::Registration(
                "Gateway account identity or XDG paths changed before its native effect".into(),
            ));
        }
        self.manager_factory
            .verify_prerequisites(effective_uid)
            .map_err(GatewayError::Registration)?;
        verify_systemd_authority(manager, paths, unit).map_err(GatewayError::Registration)
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
            return Err(pre_admission("Gateway service configuration stage changed"));
        }
        let (effective_uid, real_uid) = self.runtime_context.user_ids();
        if effective_uid != real_uid {
            return Err(pre_admission(
                "The packaged gateway refuses a set-user-ID process",
            ));
        }
        let unit = unit_name(stage, self.configuration.instance()).map_err(pre_admission)?;
        let paths = self.paths(&unit).map_err(pre_admission)?;
        let fingerprint = runtime_fingerprint(runtime).map_err(pre_admission)?;
        let manager = self
            .manager_factory
            .connect(effective_uid)
            .map_err(pre_admission)?;
        verify_systemd_authority(manager.as_ref(), &paths, &unit).map_err(pre_admission)?;
        let installed = manager.snapshot(&unit).map_err(pre_admission)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            stage,
            self.configuration.instance(),
        );
        let advertisement = self
            .runtime_context
            .observe_endpoint_health(&data)
            .map_err(pre_admission)?;
        let advertisement = advertisement
            .as_ref()
            .map(LinuxEndpointHealth::advertisement);
        let before = portable_incarnation(&unit, installed.as_ref(), advertisement)
            .map_err(pre_admission)?;
        let (prior_definition, startup_failure) = match (installed.as_ref(), before.as_ref()) {
            (Some(snapshot), Some(prior)) => {
                native_from_snapshot(snapshot, prior.target(), &unit, true)
                    .map_err(pre_admission)?;
                let prior_path = snapshot
                    .exec_start_ex
                    .first()
                    .and_then(|entry| environment_value(&entry.1, "NESSA_AGENT_PATH"))
                    .ok_or_else(|| {
                        pre_admission("The running systemd gateway has no retained agent path")
                    })
                    .and_then(|path| {
                        SearchPath::parse(path).map_err(|error| {
                            pre_admission(format!(
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
                .map_err(pre_admission)?;
                if !snapshot_matches(snapshot, manager.as_ref(), &paths, &unit, &prior_rendered)
                    .map_err(pre_admission)?
                    || manager
                        .get_unit_by_pid(prior.process_id())
                        .map_err(pre_admission)?
                        != snapshot.object_path
                {
                    return Err(pre_admission(
                        "The running systemd gateway lacks exact owned unit identity",
                    ));
                }
                (Some(prior_rendered.bytes), None)
            }
            (Some(snapshot), None) => {
                let arguments = snapshot.exec_start_ex.first().map(|entry| &entry.1);
                let fingerprint = arguments
                    .and_then(|values| environment_value(values, "NESSA_RUNTIME_FINGERPRINT"));
                let generation = arguments
                    .and_then(|values| environment_value(values, "NESSA_SERVICE_GENERATION"));
                let agent = arguments
                    .and_then(|values| environment_value(values, "NESSA_AGENT_PATH"))
                    .and_then(|value| SearchPath::parse(value).ok());
                let authority = match (fingerprint, generation, agent) {
                    (Some(fingerprint), Some(generation), Some(agent))
                        if digest(fingerprint) && digest(generation) =>
                    {
                        let installed_target = ReconciliationTarget::new(
                            unit.as_str().into(),
                            fingerprint.into(),
                            generation.into(),
                        )
                        .map_err(pre_admission)?;
                        let rendered = render(UnitDefinition {
                            unit: &unit,
                            runtime: &paths.runtime_root.join(fingerprint),
                            configuration: &self.configuration,
                            data: &data,
                            home: &self.home,
                            agent_path: &agent,
                            fingerprint,
                            generation,
                        })
                        .map_err(pre_admission)?;
                        let record = recorded_failure(&data.join("logs"));
                        match record.and_then(|record| {
                            inactive_startup_failure_authority(snapshot, &record, installed_target)
                        }) {
                            Some(authority)
                                if installed_definition_matches(
                                    snapshot,
                                    manager.as_ref(),
                                    &paths,
                                    &unit,
                                    &rendered,
                                )? =>
                            {
                                Some((rendered.bytes, authority))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let Some((bytes, authority)) = authority else {
                    return Err(pre_admission(
                        "An installed systemd unit has no corroborated managed endpoint; it was preserved",
                    ));
                };
                (Some(bytes), Some(authority))
            }
            _ => (None, None),
        };
        let staged_runtime = paths.runtime_root.join(&fingerprint);
        let path = chosen_agent_path(agent_path, installed.as_ref(), &staged_runtime)
            .map_err(pre_admission)?;
        let generation = reusable_generation(installed.as_ref(), &fingerprint)
            .unwrap_or(random_digest().map_err(pre_admission)?);
        let target = ReconciliationTarget::new(
            unit.as_str().to_owned(),
            fingerprint.clone(),
            generation.clone(),
        )
        .map_err(pre_admission)?;
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
        .map_err(pre_admission)?;
        let rendered_digest = definition_digest(&rendered.bytes);

        self.revalidate_preflight(
            manager.as_ref(),
            PreflightEvidence {
                runtime,
                fingerprint: &fingerprint,
                unit: &unit,
                paths: &paths,
                installed: installed.as_ref(),
                data: &data,
                advertisement,
                startup_failure: startup_failure.as_ref(),
            },
        )
        .map_err(pre_admission)?;

        let intent = match startup_failure {
            Some(authority) => GatewayReconciliationIntent::with_startup_failure(
                attempt.clone(),
                target.clone(),
                authority,
            )?,
            None => {
                GatewayReconciliationIntent::new(attempt.clone(), target.clone(), before.clone())?
            }
        };
        progress.intent_admitted(intent)?;

        if let Some(ready) = exact_ready(
            manager.as_ref(),
            &paths,
            &unit,
            &target,
            &rendered,
            advertisement,
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
                RetirementAuthority {
                    configuration: &self.configuration,
                    data: &data,
                    home: &self.home,
                    paths: &paths,
                    process_factory: self.process_factory.as_ref(),
                },
                LinuxRuntime {
                    manager: manager.as_ref(),
                    context: self.runtime_context.as_ref(),
                    clock: self.clock.as_ref(),
                },
            )?;
            progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
            self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)?;
            stop_unit(
                progress,
                &unit,
                &target,
                &data,
                &paths,
                LinuxRuntime {
                    manager: manager.as_ref(),
                    context: self.runtime_context.as_ref(),
                    clock: self.clock.as_ref(),
                },
            )?;
            progress.history_observed(ReconciliationHistoryFact::OldServiceUnloaded);
        }

        verify_systemd_authority(manager.as_ref(), &paths, &unit)
            .map_err(GatewayError::Registration)?;

        stage_runtime(
            progress,
            runtime,
            &paths.runtime_root,
            &staged_runtime,
            &fingerprint,
            || {
                self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                    .map_err(|error| error.to_string())
            },
        )?;
        let data_directory_generation = random_digest().map_err(GatewayError::Registration)?;
        planned_directory(
            progress,
            "create-gateway-data-directory",
            LifecycleEffect::CreateGatewayDataDirectory {
                target: target.clone(),
            },
            LifecycleEffect::SettleGatewayDataDirectoryTransaction {
                target: target.clone(),
                generation: data_directory_generation.clone(),
            },
            &data,
            &data_directory_generation,
            || {
                self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                    .map_err(|error| error.to_string())
            },
        )?;
        planned_physical_with_cleanup(
            progress,
            "publish-systemd-unit",
            PhysicalPlanWithCleanup {
                primary: LifecycleEffect::PublishSystemdServiceDefinition {
                    target: target.clone(),
                    definition_digest: rendered_digest.clone(),
                },
                cleanup: LifecycleEffect::SettleSystemdDefinitionTransaction {
                    target: target.clone(),
                    definition_digest: rendered_digest,
                },
            },
            PhysicalActionsWithCleanup {
                authorize: || {
                    self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                        .map_err(|error| error.to_string())
                },
                run: || match prior_definition.as_ref() {
                    Some(prior) => replace_owned_bytes(
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
                present: || bytes_match(&paths.unit_file, &rendered.bytes),
                cleanup_run: |_| {
                    settle_definition_transaction(
                        &paths.unit_file,
                        &rendered.bytes,
                        target.service_generation(),
                    )
                    .map(|_| ())
                },
                cleanup_present: || {
                    let transaction =
                        definition_transaction(&paths.unit_file, target.service_generation())?;
                    Ok(transaction.publish_temporary.is_some()
                        || transaction.replace_temporary.is_some())
                },
            },
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
            || {
                self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                    .map_err(|error| error.to_string())
            },
            || manager.reload(),
            || Ok(true),
        )?;
        let wants_directory_generation = random_digest().map_err(GatewayError::Registration)?;
        planned_directory(
            progress,
            "create-systemd-wants-directory",
            LifecycleEffect::CreateSystemdWantsDirectory {
                target: target.clone(),
            },
            LifecycleEffect::SettleSystemdWantsDirectoryTransaction {
                target: target.clone(),
                generation: wants_directory_generation.clone(),
            },
            &paths.wants_directory,
            &wants_directory_generation,
            || {
                self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                    .map_err(|error| error.to_string())
            },
        )?;
        planned_physical_with_cleanup(
            progress,
            "publish-systemd-wants-link",
            PhysicalPlanWithCleanup {
                primary: LifecycleEffect::PublishSystemdWantsLink {
                    target: target.clone(),
                },
                cleanup: LifecycleEffect::SettleSystemdWantsLinkTransaction {
                    target: target.clone(),
                },
            },
            PhysicalActionsWithCleanup {
                authorize: || {
                    self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)
                        .map_err(|error| error.to_string())
                },
                run: || {
                    publish_wants_link(
                        &paths.wants_directory,
                        &paths.wants_link,
                        &paths.unit_file,
                        target.service_generation(),
                    )
                },
                present: || wants_link_matches(&paths.wants_link, &paths.unit_file),
                cleanup_run: |_| {
                    discard_wants_link_temporary(
                        &paths.wants_directory,
                        &paths.wants_link,
                        &paths.unit_file,
                        target.service_generation(),
                    )
                },
                cleanup_present: || {
                    wants_link_temporary_present(
                        &paths.wants_directory,
                        target.service_generation(),
                    )
                },
            },
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
        self.revalidate_effect_authority(manager.as_ref(), &unit, &paths)?;
        start_unit(
            progress,
            &unit,
            &target,
            &data,
            &paths,
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
        verify_systemd_authority(manager.as_ref(), &paths, &unit)
            .map_err(GatewayError::Registration)?;
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        if recovery.pending_step().is_some_and(|step| {
            matches!(
                step.step().effect(),
                LifecycleEffect::PublishSystemdServiceDefinition { target, .. }
                    | LifecycleEffect::SettleSystemdDefinitionTransaction { target, .. }
                    if target == recovery.target()
            )
        }) {
            let definition_digest = match recovery.pending_step().unwrap().step().effect() {
                LifecycleEffect::PublishSystemdServiceDefinition {
                    definition_digest, ..
                }
                | LifecycleEffect::SettleSystemdDefinitionTransaction {
                    definition_digest, ..
                } => definition_digest,
                _ => unreachable!(),
            };
            settle_recovered_definition(&paths, recovery.target(), definition_digest)
                .map_err(GatewayError::Registration)?;
        }
        if let Some(step) = recovery.pending_step() {
            match step.step().effect() {
                LifecycleEffect::PublishSystemdWantsLink { target }
                    if target == recovery.target() =>
                {
                    settle_wants_link_transaction(
                        &paths.wants_directory,
                        &paths.wants_link,
                        &paths.unit_file,
                        recovery.target().service_generation(),
                    )
                    .map_err(GatewayError::Registration)?;
                }
                LifecycleEffect::SettleSystemdWantsLinkTransaction { target }
                    if target == recovery.target() =>
                {
                    discard_wants_link_temporary(
                        &paths.wants_directory,
                        &paths.wants_link,
                        &paths.unit_file,
                        recovery.target().service_generation(),
                    )
                    .map_err(GatewayError::Registration)?;
                }
                LifecycleEffect::CreateGatewayDataDirectory { target }
                    if target == recovery.target() =>
                {
                    let generation = directory_cleanup_generation(recovery, true)?;
                    settle_recovered_owned_directory_transaction(
                        &data,
                        generation,
                        matches!(step.completion(), Some(LifecycleCommandResult::Accepted)),
                    )
                    .map_err(GatewayError::Registration)?;
                }
                LifecycleEffect::CreateSystemdWantsDirectory { target }
                    if target == recovery.target() =>
                {
                    let generation = directory_cleanup_generation(recovery, false)?;
                    settle_recovered_owned_directory_transaction(
                        &paths.wants_directory,
                        generation,
                        matches!(step.completion(), Some(LifecycleCommandResult::Accepted)),
                    )
                    .map_err(GatewayError::Registration)?;
                }
                LifecycleEffect::SettleGatewayDataDirectoryTransaction { target, generation }
                    if target == recovery.target() =>
                {
                    settle_recovered_owned_directory_transaction(&data, generation, true)
                        .map_err(GatewayError::Registration)?;
                }
                LifecycleEffect::SettleSystemdWantsDirectoryTransaction { target, generation }
                    if target == recovery.target() =>
                {
                    settle_recovered_owned_directory_transaction(
                        &paths.wants_directory,
                        generation,
                        true,
                    )
                    .map_err(GatewayError::Registration)?;
                }
                _ => {}
            }
        }
        let observed = observe_systemd_state(
            manager.as_ref(),
            &unit,
            &data,
            self.runtime_context.as_ref(),
        )
        .map_err(GatewayError::Registration)?;
        let broad_artifact_present = exact_target_artifact_present(
            &paths,
            &unit,
            recovery.target(),
            observed.snapshot.as_ref(),
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
                RecoveryPhysical {
                    manager: manager.identity(),
                    snapshot: observed.snapshot.as_ref(),
                    incarnation: observed.incarnation.as_ref(),
                    target_artifact_present: broad_artifact_present,
                },
                DefinitionAuthority {
                    configuration: &self.configuration,
                    data: &data,
                    home: &self.home,
                },
            )
            .map_err(GatewayError::Registration)?;
            let observation = observed.lifecycle_observation(
                recovery
                    .latest_observation()
                    .map_or(1, |value| value.version().saturating_add(1)),
                artifact_present,
            )?;
            let source = step.source();
            retry(|| journal.observation(&source, &observation))?;
            observation
        } else if let Some(observation) = recovery.latest_observation() {
            if observation.incarnation() != observed.incarnation.as_ref()
                || observation.target_artifact_present() != broad_artifact_present
                || observation.systemd() != observed.native.as_ref()
                || observation.systemd_state() != Some(observed.unit_state)
            {
                return Err(GatewayError::Registration(
                    "Fresh systemd recovery state disagrees with its last durable observation"
                        .into(),
                ));
            }
            observation.clone()
        } else {
            let observation = if let Some(authority) = recovery.startup_failure() {
                if recovery.has_effect_plan()
                    || observed.incarnation.is_some()
                    || !matches!(
                        observed.unit_state,
                        SystemdUnitState::Absent | SystemdUnitState::Inactive
                    )
                {
                    return Err(GatewayError::Registration(
                        "The unresolved startup-failure recovery lacks an exact safe closure"
                            .into(),
                    ));
                }
                if observed.unit_state == SystemdUnitState::Absent {
                    LifecycleObservation::with_systemd_state(1, false, SystemdUnitState::Absent)
                        .map_err(|error| GatewayError::Registration(error.to_string()))?
                } else {
                    let retained = exact_target_artifact_present(
                        &paths,
                        &unit,
                        authority.target(),
                        observed.snapshot.as_ref(),
                        DefinitionAuthority {
                            configuration: &self.configuration,
                            data: &data,
                            home: &self.home,
                        },
                    )
                    .map_err(GatewayError::Registration)?;
                    if !retained {
                        return Err(GatewayError::Registration(
                            "The unresolved startup-failure authority target is not retained exactly".into(),
                        ));
                    }
                    if recorded_failure(&data.join("logs"))
                        .and_then(|record| record.authority(authority.target().clone()))
                        .as_ref()
                        != Some(authority)
                    {
                        return Err(GatewayError::Registration(
                            "The unresolved startup-failure record no longer matches its authority"
                                .into(),
                        ));
                    }
                    if !observed.snapshot.as_ref().is_some_and(|snapshot| {
                        inactive_systemd_exit_matches(snapshot, authority.process_id())
                    }) {
                        return Err(GatewayError::Registration(
                            "The unresolved startup-failure systemd exit no longer matches its authority".into(),
                        ));
                    }
                    if authority.target() == recovery.target() {
                        LifecycleObservation::with_systemd_state(
                            1,
                            true,
                            SystemdUnitState::Inactive,
                        )
                        .map_err(|error| GatewayError::Registration(error.to_string()))?
                    } else {
                        LifecycleObservation::with_retained_systemd_target(
                            1,
                            authority.target().clone(),
                        )
                    }
                }
            } else {
                if recovery.has_effect_plan()
                    || observed.incarnation.as_ref() != recovery.before()
                    || broad_artifact_present
                    || !matches!(
                        observed.unit_state,
                        SystemdUnitState::Absent | SystemdUnitState::Inactive
                    )
                {
                    return Err(GatewayError::Registration(
                        "The unresolved systemd lifecycle lacks an exact safe closure".into(),
                    ));
                }
                observed.lifecycle_observation(1, false)?
            };
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
        verify_systemd_authority(manager.as_ref(), &paths, &unit).map_err(GatewayError::Stop)?;
        let target = intended.audit_identity()?.target().clone();
        let data = data_directory_path(
            self.configuration.data_root(),
            self.configuration.stage(),
            self.configuration.instance(),
        );
        let snapshot = manager
            .snapshot(&unit)
            .map_err(GatewayError::Stop)?
            .ok_or_else(|| GatewayError::Stop("The intended systemd unit is absent".into()))?;
        // Hold the exact process from snapshot A before any corroborating read.
        // Every later proof refers to this descriptor, so PID reuse cannot
        // redirect the eventual signal.
        let process = self
            .process_factory
            .open(snapshot.main_process_id)
            .map_err(|error| {
                GatewayError::Stop(format!(
                    "The gateway process cannot be held for exact signal delivery: {error}"
                ))
            })?;
        let health = self
            .runtime_context
            .observe_endpoint_health(&data)
            .map_err(GatewayError::Stop)?;
        let advertisement = health.as_ref().map(LinuxEndpointHealth::advertisement);
        let native =
            native_from_snapshot(&snapshot, &target, &unit, true).map_err(GatewayError::Stop)?;
        let portable =
            portable_incarnation(&unit, Some(&snapshot), advertisement)?.ok_or_else(|| {
                GatewayError::Stop(
                    "The intended systemd process has no matching endpoint advertisement".into(),
                )
            })?;
        if portable != intended.audit_identity()? {
            return Err(GatewayError::Stop(
                "The current systemd endpoint is not the intended gateway incarnation".into(),
            ));
        }
        let retained_path = snapshot
            .exec_start_ex
            .first()
            .and_then(|entry| environment_value(&entry.1, "NESSA_AGENT_PATH"))
            .ok_or_else(|| GatewayError::Stop("The intended unit has no agent path".into()))
            .and_then(|path| {
                SearchPath::parse(path).map_err(|error| GatewayError::Stop(error.to_string()))
            })?;
        let rendered = render(UnitDefinition {
            unit: &unit,
            runtime: &paths.runtime_root.join(target.runtime_fingerprint()),
            configuration: &self.configuration,
            data: &data,
            home: &self.home,
            agent_path: &retained_path,
            fingerprint: target.runtime_fingerprint(),
            generation: target.service_generation(),
        })
        .map_err(GatewayError::Stop)?;
        if !snapshot_matches(&snapshot, manager.as_ref(), &paths, &unit, &rendered)
            .map_err(|error| GatewayError::Stop(error.to_string()))?
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
        let refreshed_health = self
            .runtime_context
            .observe_endpoint_health(&data)
            .map_err(GatewayError::Stop)?;
        require_same_endpoint_health(health.as_ref(), refreshed_health.as_ref())
            .map_err(GatewayError::Stop)?;
        let second = manager
            .snapshot(&unit)
            .map_err(GatewayError::Stop)?
            .ok_or_else(|| GatewayError::Stop("The intended systemd unit disappeared".into()))?;
        if second != snapshot {
            return Err(GatewayError::Stop(
                "The intended systemd process changed during proof".into(),
            ));
        }
        verify_systemd_authority(manager.as_ref(), &paths, &unit).map_err(GatewayError::Stop)?;
        let command = LinuxSignalAuthority::corroborate(process, token, native, portable, 1)?
            .dispatch(session, plan, libc::SIGUSR1)?;
        let command_record = session.command_result(command.clone());
        let observation = observe_systemd_state(
            manager.as_ref(),
            &unit,
            &data,
            self.runtime_context.as_ref(),
        )
        .and_then(|observed| {
            observed
                .lifecycle_observation(2, wants_link_matches(&paths.wants_link, &paths.unit_file)?)
                .map_err(|error| error.to_string())
        });
        let fresh_observation = observation
            .as_ref()
            .map_err(|error| error.clone())
            .and_then(|observation| {
                session
                    .fresh_observation(observation.clone())
                    .map_err(|error| error.to_string())
            });
        let completion_delivery = retry(|| {
            journal.effect_completion("stop-agents-on-desktop-quit", "signal-agents", &command)
        });
        let physical_failures = command_failure(&command)
            .into_iter()
            .chain(observation.as_ref().err().cloned())
            .chain(fresh_observation.as_ref().err().cloned())
            .collect::<Vec<_>>();
        if let Err(error) = command_record {
            return Err(audit_with_physical(
                error,
                physical_failures.clone().into_iter(),
            ));
        }
        if let Err(audit) = completion_delivery {
            return Err(audit_with_physical(
                audit,
                physical_failures.clone().into_iter(),
            ));
        }
        let observation = observation.map_err(GatewayError::Stop)?;
        fresh_observation.map_err(GatewayError::Stop)?;
        if let Err(audit) = retry(|| {
            journal.observation(
                &LifecycleObservationSource::Effect {
                    plan_id: "stop-agents-on-desktop-quit".into(),
                    step_id: "signal-agents".into(),
                },
                &observation,
            )
        }) {
            return Err(audit_with_physical(audit, physical_failures.into_iter()));
        }
        match command {
            LifecycleCommandResult::Accepted => Ok(observation),
            LifecycleCommandResult::Rejected(message)
            | LifecycleCommandResult::Failed(message)
            | LifecycleCommandResult::Indeterminate(message) => Err(GatewayError::Stop(message)),
        }
    }
}

fn directory_cleanup_generation(
    recovery: &GatewayLifecycleRecovery,
    data_directory: bool,
) -> Result<&str, GatewayError> {
    recovery
        .pending_step()
        .and_then(|step| {
            step.contingencies()
                .iter()
                .find_map(|contingency| match contingency.effect() {
                    LifecycleEffect::SettleGatewayDataDirectoryTransaction {
                        target,
                        generation,
                    } if data_directory && target == recovery.target() => Some(generation.as_str()),
                    LifecycleEffect::SettleSystemdWantsDirectoryTransaction {
                        target,
                        generation,
                    } if !data_directory && target == recovery.target() => {
                        Some(generation.as_str())
                    }
                    _ => None,
                })
        })
        .ok_or_else(|| {
            GatewayError::Registration(
                "Recovered directory creation lacks its exact cleanup transaction".into(),
            )
        })
}

fn command_failure(result: &LifecycleCommandResult) -> Option<String> {
    match result {
        LifecycleCommandResult::Accepted => None,
        LifecycleCommandResult::Rejected(error)
        | LifecycleCommandResult::Failed(error)
        | LifecycleCommandResult::Indeterminate(error) => Some(error.clone()),
    }
}

fn require_same_endpoint_health(
    initial: Option<&LinuxEndpointHealth>,
    refreshed: Option<&LinuxEndpointHealth>,
) -> Result<(), String> {
    (initial.is_some() && initial == refreshed)
        .then_some(())
        .ok_or_else(|| "The gateway health identity changed during proof".into())
}

fn stage_runtime(
    progress: &dyn GatewayReconciliationProgress,
    source: &Path,
    root: &Path,
    destination: &Path,
    fingerprint: &str,
    authorize: impl FnOnce() -> Result<(), String>,
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
    let result = authorize()
        .and_then(|()| publish_runtime(source, root, fingerprint, &staging_generation).map(|_| ()));
    let primary_present = runtime_artifact_present(destination, fingerprint);
    let cleanup_result = remove_staging_runtime(root, &staging_generation);
    let cleanup_present = staging_runtime_present(root, &staging_generation);
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(audit) = progress.effect_completed(plan_id, primary.id(), &completion) {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(primary_present.as_ref().err())
                .chain(cleanup_result.as_ref().err())
                .chain(cleanup_present.as_ref().err())
                .cloned(),
        ));
    }
    let primary_present = primary_present.map_err(GatewayError::Registration)?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: primary.id().into(),
        },
        None,
        primary_present,
    ) {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(cleanup_result.as_ref().err())
                .chain(cleanup_present.as_ref().err())
                .cloned(),
        ));
    }
    let cleanup_completion = match &cleanup_result {
        Ok(true) => LifecycleCommandResult::Accepted,
        Ok(false) => LifecycleCommandResult::Indeterminate(
            "The exact staging runtime was already absent after the primary returned".into(),
        ),
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(audit) = progress.effect_completed(plan_id, cleanup.id(), &cleanup_completion) {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(cleanup_result.as_ref().err())
                .chain(cleanup_present.as_ref().err())
                .cloned(),
        ));
    }
    let cleanup_present = cleanup_present.map_err(GatewayError::Registration)?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: cleanup.id().into(),
        },
        None,
        cleanup_present,
    ) {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(cleanup_result.as_ref().err())
                .cloned(),
        ));
    }
    cleanup_result.map_err(GatewayError::Registration)?;
    result.map_err(GatewayError::Registration)
}

fn planned_directory(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    primary: LifecycleEffect,
    cleanup: LifecycleEffect,
    path: &Path,
    generation: &str,
    authorize: impl FnOnce() -> Result<(), String>,
) -> Result<(), GatewayError> {
    let transaction = Mutex::new(None);
    planned_physical_with_cleanup(
        progress,
        plan_id,
        PhysicalPlanWithCleanup { primary, cleanup },
        PhysicalActionsWithCleanup {
            authorize,
            run: || {
                let outcome = create_owned_directory_transaction(path, generation);
                *transaction
                    .lock()
                    .map_err(|_| "Gateway directory transaction lock was poisoned".to_string())? =
                    outcome.transaction;
                outcome.result
            },
            present: || owned_directory_present(path),
            cleanup_run: |retain| {
                let transaction = transaction
                    .lock()
                    .map_err(|_| "Gateway directory transaction lock was poisoned".to_string())?;
                settle_owned_directory_transaction(transaction.as_ref(), retain)
            },
            cleanup_present: || owned_directory_transaction_present(path, generation),
        },
    )
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
    let observed = present();
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(audit) = progress.effect_completed(plan_id, step.id(), &completion) {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(observed.as_ref().err())
                .cloned(),
        ));
    }
    let observed = observed.map_err(GatewayError::Registration)?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: step.id().into(),
        },
        None,
        observed,
    ) {
        return Err(audit_with_physical(
            audit,
            result.as_ref().err().into_iter().cloned(),
        ));
    }
    result.map_err(GatewayError::Registration)
}

struct PhysicalPlanWithCleanup {
    primary: LifecycleEffect,
    cleanup: LifecycleEffect,
}

struct PhysicalActionsWithCleanup<Authorize, Run, Present, CleanupRun, CleanupPresent> {
    authorize: Authorize,
    run: Run,
    present: Present,
    cleanup_run: CleanupRun,
    cleanup_present: CleanupPresent,
}

fn planned_physical_with_cleanup<Authorize, Run, Present, CleanupRun, CleanupPresent>(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    plan: PhysicalPlanWithCleanup,
    actions: PhysicalActionsWithCleanup<Authorize, Run, Present, CleanupRun, CleanupPresent>,
) -> Result<(), GatewayError>
where
    Authorize: FnOnce() -> Result<(), String>,
    Run: FnOnce() -> Result<(), String>,
    Present: FnOnce() -> Result<bool, String>,
    CleanupRun: FnOnce(bool) -> Result<(), String>,
    CleanupPresent: FnOnce() -> Result<bool, String>,
{
    let primary = LifecyclePlanStep::new(
        "primary".into(),
        plan.primary,
        LifecycleEffectPredicate::Always,
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let cleanup = LifecyclePlanStep::new(
        "settle-transaction".into(),
        plan.cleanup,
        LifecycleEffectPredicate::PrimaryReturned,
    )
    .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &primary, std::slice::from_ref(&cleanup))?;
    let result = (actions.authorize)().and_then(|()| (actions.run)());
    let primary_present = (actions.present)();
    let cleanup_result = (actions.cleanup_run)(result.is_ok());
    let cleanup_present = (actions.cleanup_present)();
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    let physical_failures = result
        .as_ref()
        .err()
        .into_iter()
        .chain(primary_present.as_ref().err())
        .chain(cleanup_result.as_ref().err())
        .chain(cleanup_present.as_ref().err())
        .cloned()
        .collect::<Vec<_>>();
    if let Err(audit) = progress.effect_completed(plan_id, primary.id(), &completion) {
        return Err(audit_with_physical(
            audit,
            physical_failures.clone().into_iter(),
        ));
    }
    let primary_present = primary_present.map_err(GatewayError::Registration)?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: primary.id().into(),
        },
        None,
        primary_present,
    ) {
        return Err(audit_with_physical(
            audit,
            physical_failures.clone().into_iter(),
        ));
    }
    let cleanup_completion = match &cleanup_result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    if let Err(audit) = progress.effect_completed(plan_id, cleanup.id(), &cleanup_completion) {
        return Err(audit_with_physical(
            audit,
            physical_failures.clone().into_iter(),
        ));
    }
    let cleanup_present = cleanup_present.map_err(GatewayError::Registration)?;
    if let Err(audit) = progress.physical_observed(
        &LifecycleObservationSource::Effect {
            plan_id: plan_id.into(),
            step_id: cleanup.id().into(),
        },
        None,
        cleanup_present,
    ) {
        return Err(audit_with_physical(audit, physical_failures.into_iter()));
    }
    cleanup_result.map_err(GatewayError::Registration)?;
    result.map_err(GatewayError::Registration)
}

fn audit_with_physical(
    audit: GatewayError,
    failures: impl Iterator<Item = String>,
) -> GatewayError {
    let failures = failures.collect::<Vec<_>>();
    let audit = match audit {
        GatewayError::Audit { audit, .. } => audit,
        error => error.to_string(),
    };
    let physical = if failures.is_empty() {
        GatewayPhysicalResult::Succeeded
    } else {
        GatewayPhysicalResult::Failed(Box::new(GatewayError::Registration(failures.join("; "))))
    };
    GatewayError::Audit {
        audit,
        physical: Some(physical),
    }
}

fn recovery_artifact_present(
    effect: &LifecycleEffect,
    paths: &LinuxGatewayPaths,
    target: &ReconciliationTarget,
    physical: RecoveryPhysical<'_>,
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
        LifecycleEffect::PublishSystemdServiceDefinition {
            target: planned,
            definition_digest: planned_digest,
        } => {
            if planned != target {
                return Err("Recovered definition plan targets another lifecycle".into());
            }
            Ok(owned_file_bytes(&paths.unit_file)?
                .is_some_and(|bytes| definition_digest(&bytes) == *planned_digest))
        }
        LifecycleEffect::SettleSystemdDefinitionTransaction {
            target: planned,
            definition_digest: planned_digest,
        } => {
            if planned != target {
                return Err("Recovered definition cleanup targets another lifecycle".into());
            }
            let transaction =
                definition_transaction(&paths.unit_file, target.service_generation())?;
            if transaction
                .current
                .as_deref()
                .is_some_and(|bytes| definition_digest(bytes) != *planned_digest)
            {
                return Err(
                    "Recovered definition cleanup disagrees with the admitted digest".into(),
                );
            }
            Ok(transaction.publish_temporary.is_some() || transaction.replace_temporary.is_some())
        }
        LifecycleEffect::CreateGatewayDataDirectory { target: planned } => {
            if planned != target {
                return Err("Recovered data-directory plan targets another lifecycle".into());
            }
            owned_directory_present(authority.data)
        }
        LifecycleEffect::SettleGatewayDataDirectoryTransaction {
            target: planned,
            generation,
        } => {
            if planned != target {
                return Err("Recovered data-directory cleanup targets another lifecycle".into());
            }
            owned_directory_transaction_present(authority.data, generation)
        }
        LifecycleEffect::CreateSystemdWantsDirectory { .. } => {
            owned_directory_present(&paths.wants_directory)
        }
        LifecycleEffect::SettleSystemdWantsDirectoryTransaction { generation, .. } => {
            owned_directory_transaction_present(&paths.wants_directory, generation)
        }
        LifecycleEffect::PublishSystemdWantsLink { .. } => {
            wants_link_matches(&paths.wants_link, &paths.unit_file)
        }
        LifecycleEffect::SettleSystemdWantsLinkTransaction { .. } => {
            wants_link_temporary_present(&paths.wants_directory, target.service_generation())
        }
        LifecycleEffect::ReloadSystemdManager { manager, unit } => match physical.snapshot {
            Some(state)
                if state.manager == *manager
                    && unit.as_str() == target.service()
                    && state.id == unit.as_str()
                    && state.names == [unit.as_str()]
                    && snapshot_declares_target(state, target) =>
            {
                Ok(true)
            }
            None => Ok(false),
            Some(_) => Err("Recovered manager reload evidence disagrees with its plan".into()),
        },
        LifecycleEffect::StartSystemdUnit {
            manager,
            unit,
            mode,
        }
        | LifecycleEffect::StopSystemdUnit {
            manager,
            unit,
            mode,
        } => {
            if manager != physical.manager
                || unit.as_str() != target.service()
                || *mode != SystemdJobMode::Fail
            {
                return Err("Recovered systemd job plan disagrees with current authority".into());
            }
            Ok(physical.target_artifact_present)
        }
        LifecycleEffect::BootstrapService { .. }
        | LifecycleEffect::AdoptReadyIncarnation { .. }
        | LifecycleEffect::UnloadService { .. } => {
            Err("A non-systemd service effect cannot be recovered by the Linux host".into())
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
            retirement_artifact_present(
                authority.data,
                request_id,
                incarnation,
                target,
                physical.incarnation,
            )
        }
        LifecycleEffect::StopAgents { incarnation } => {
            Ok(physical.incarnation == Some(incarnation))
        }
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
    planned_digest: &str,
) -> Result<(), String> {
    let transaction = definition_transaction(&paths.unit_file, target.service_generation())?;
    if transaction.publish_temporary.is_none() && transaction.replace_temporary.is_none() {
        return transaction
            .current
            .as_deref()
            .is_some_and(|bytes| definition_digest(bytes) == planned_digest)
            .then_some(())
            .ok_or_else(|| {
                "Settled definition does not match its admitted journal digest".to_string()
            });
    }
    let candidates = [
        transaction.current.as_deref(),
        transaction.publish_temporary.as_deref(),
        transaction.replace_temporary.as_deref(),
    ];
    let mut expected = None;
    for candidate in candidates.into_iter().flatten() {
        if definition_digest(candidate) == planned_digest {
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

fn definition_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn snapshot_declares_target(snapshot: &UnitSnapshot, target: &ReconciliationTarget) -> bool {
    snapshot.exec_start_ex.first().is_some_and(|entry| {
        environment_value(&entry.1, "NESSA_RUNTIME_FINGERPRINT")
            == Some(target.runtime_fingerprint())
            && environment_value(&entry.1, "NESSA_SERVICE_GENERATION")
                == Some(target.service_generation())
    })
}

fn systemd_target_artifact_present(
    manager: &SystemdManagerIdentity,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    snapshot: Option<&UnitSnapshot>,
) -> Result<bool, String> {
    Ok(snapshot.is_some_and(|state| {
        state.manager == *manager
            && state.id == unit.as_str()
            && state.names == [unit.as_str()]
            && snapshot_declares_target(state, target)
    }))
}

fn systemd_job_reached_state(
    operation: SystemdJobOperation,
    target: &ReconciliationTarget,
    state: &ObservedSystemdState,
) -> bool {
    match operation {
        SystemdJobOperation::Start => {
            state.unit_state == SystemdUnitState::Active
                && state
                    .incarnation
                    .as_ref()
                    .is_some_and(|incarnation| incarnation.target() == target)
                && state.native.is_some()
        }
        SystemdJobOperation::Stop => matches!(
            state.unit_state,
            SystemdUnitState::Absent | SystemdUnitState::Inactive
        ),
    }
}

fn start_unit(
    progress: &dyn GatewayReconciliationProgress,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    data: &Path,
    paths: &LinuxGatewayPaths,
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
        SystemdJobAuthority {
            manager: runtime.manager,
            unit,
            target,
            operation: SystemdJobOperation::Start,
            data,
            paths,
            runtime_context: runtime.context,
            clock: runtime.clock,
        },
    )
}

fn stop_unit(
    progress: &dyn GatewayReconciliationProgress,
    unit: &SystemdUnitName,
    target: &ReconciliationTarget,
    data: &Path,
    paths: &LinuxGatewayPaths,
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
        SystemdJobAuthority {
            manager: runtime.manager,
            unit,
            target,
            operation: SystemdJobOperation::Stop,
            data,
            paths,
            runtime_context: runtime.context,
            clock: runtime.clock,
        },
    )
}

fn run_systemd_job(
    progress: &dyn GatewayReconciliationProgress,
    plan_id: &str,
    effect: LifecycleEffect,
    authority: SystemdJobAuthority<'_>,
) -> Result<(), GatewayError> {
    let step = LifecyclePlanStep::new("primary".into(), effect, LifecycleEffectPredicate::Always)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    progress.effect_planned(plan_id, &step, &[])?;
    verify_systemd_authority(authority.manager, authority.paths, authority.unit)
        .map_err(GatewayError::Registration)?;
    let handle = match authority.manager.recheck_identity().and_then(|()| {
        authority
            .manager
            .enqueue(authority.operation, authority.unit, authority.clock)
    }) {
        Ok(handle) => handle,
        Err(error) => {
            let observed = observe_after_job(
                authority.manager,
                authority.unit,
                authority.data,
                authority.runtime_context,
            )?;
            progress.effect_completed(
                plan_id,
                step.id(),
                &LifecycleCommandResult::Indeterminate(error.clone()),
            )?;
            let artifact_present = systemd_target_artifact_present(
                authority.manager.identity(),
                authority.unit,
                authority.target,
                observed.snapshot.as_ref(),
            )
            .map_err(GatewayError::Registration)?;
            let reached =
                systemd_job_reached_state(authority.operation, authority.target, &observed);
            observed.record(
                progress,
                &LifecycleObservationSource::Effect {
                    plan_id: plan_id.into(),
                    step_id: step.id().into(),
                },
                artifact_present,
            )?;
            return reached
                .then_some(())
                .ok_or(GatewayError::Registration(error));
        }
    };
    let attempt = handle.attempt().clone();
    // Once systemd has accepted an enqueue, audit delivery cannot cancel the
    // physical settlement. Retain every delivery result, but always wait for
    // the terminal and make the fresh observation before returning it.
    let attempt_delivery = progress.native_attempt_recorded(plan_id, step.id(), &attempt);
    let terminal = handle.wait(authority.clock, authority.clock.now() + JOB_TIMEOUT);
    let completion = classify_terminal(
        authority.manager,
        authority.operation,
        authority.unit,
        &attempt,
        &terminal,
    );
    let completion_delivery = progress.effect_completed(plan_id, step.id(), &completion);
    let observed = observe_after_job(
        authority.manager,
        authority.unit,
        authority.data,
        authority.runtime_context,
    );
    let (reached, observation_delivery) = match observed {
        Ok(observed) => {
            let artifact_present = systemd_target_artifact_present(
                authority.manager.identity(),
                authority.unit,
                authority.target,
                observed.snapshot.as_ref(),
            )
            .map_err(GatewayError::Registration)?;
            let reached =
                systemd_job_reached_state(authority.operation, authority.target, &observed);
            let delivery = observed.record(
                progress,
                &LifecycleObservationSource::Effect {
                    plan_id: plan_id.into(),
                    step_id: step.id().into(),
                },
                artifact_present,
            );
            (reached, delivery)
        }
        Err(error) => (false, Err(error)),
    };
    attempt_delivery?;
    completion_delivery?;
    observation_delivery?;
    reached.then_some(()).ok_or_else(|| {
        GatewayError::Registration(format!(
            "systemd operation for {} did not reach its requested physical state after {completion:?}",
            authority.target.service()
        ))
    })
}

fn observe_after_job(
    manager: &dyn LinuxUserManager,
    unit: &SystemdUnitName,
    data: &Path,
    runtime_context: &dyn LinuxRuntimeContext,
) -> Result<ObservedSystemdState, GatewayError> {
    observe_systemd_state(manager, unit, data, runtime_context).map_err(GatewayError::Registration)
}

fn observe_systemd_state(
    manager: &dyn LinuxUserManager,
    unit: &SystemdUnitName,
    data: &Path,
    runtime_context: &dyn LinuxRuntimeContext,
) -> Result<ObservedSystemdState, String> {
    manager.recheck_identity()?;
    let snapshot = manager.snapshot(unit)?;
    let health = runtime_context
        .observe_endpoint_health(data)
        .map_err(|error| error.to_string())?;
    manager.recheck_identity()?;
    let incarnation = portable_incarnation(
        unit,
        snapshot.as_ref(),
        health.as_ref().map(LinuxEndpointHealth::advertisement),
    )
    .map_err(|error| error.to_string())?;
    let unit_state = classify_unit_state(snapshot.as_ref(), incarnation.as_ref());
    let native = match (&snapshot, &incarnation, unit_state) {
        (Some(snapshot), Some(incarnation), SystemdUnitState::Active) => Some(
            native_from_snapshot(snapshot, incarnation.target(), unit, true)?,
        ),
        (None, None, _) | (Some(_), None, _) => None,
        (None, Some(_), _) => {
            return Err("Gateway endpoint observation has no systemd unit snapshot".into())
        }
        (Some(_), Some(_), _) => {
            return Err("Gateway endpoint identity contradicts the systemd unit state".into())
        }
    };
    Ok(ObservedSystemdState {
        snapshot,
        incarnation,
        native,
        unit_state,
    })
}

fn classify_unit_state(
    snapshot: Option<&UnitSnapshot>,
    incarnation: Option<&ReconciliationIncarnation>,
) -> SystemdUnitState {
    let Some(snapshot) = snapshot else {
        return SystemdUnitState::Absent;
    };
    match (snapshot.active_state.as_str(), snapshot.sub_state.as_str()) {
        ("inactive", "dead") if snapshot.main_process_id == 0 => SystemdUnitState::Inactive,
        ("activating", _) => SystemdUnitState::Activating,
        ("active", "running") if snapshot.main_process_id != 0 && incarnation.is_some() => {
            SystemdUnitState::Active
        }
        ("deactivating", _) => SystemdUnitState::Deactivating,
        ("failed", _) => SystemdUnitState::Failed,
        _ => SystemdUnitState::Unknown,
    }
}

fn classify_terminal(
    manager: &dyn LinuxUserManager,
    operation: SystemdJobOperation,
    unit: &SystemdUnitName,
    attempt: &SystemdJobAttempt,
    terminal: &Result<JobTerminal, String>,
) -> LifecycleCommandResult {
    if let Err(error) = manager.recheck_identity() {
        return LifecycleCommandResult::Indeterminate(error);
    }
    match terminal {
        Ok(terminal) => {
            let terminal = match SystemdJobTerminal::new(
                terminal.manager.clone(),
                terminal.object_path.clone(),
                terminal.job_id,
                terminal.unit.clone(),
                terminal.result.clone(),
            ) {
                Ok(terminal) => terminal,
                Err(error) => return LifecycleCommandResult::Indeterminate(error.to_string()),
            };
            match attempt.classify_terminal(
                manager.identity(),
                operation,
                SystemdJobMode::Fail,
                unit,
                &terminal,
            ) {
                SystemdJobConclusion::Accepted => LifecycleCommandResult::Accepted,
                SystemdJobConclusion::Rejected(error) => LifecycleCommandResult::Rejected(error),
                SystemdJobConclusion::Indeterminate(error) => {
                    LifecycleCommandResult::Indeterminate(error)
                }
            }
        }
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
        let health = runtime
            .context
            .observe_endpoint_health(data)
            .map_err(GatewayError::Registration)?;
        if let Some(ready) = exact_ready(
            runtime.manager,
            paths,
            unit,
            target,
            rendered,
            health.as_ref().map(LinuxEndpointHealth::advertisement),
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

fn installed_definition_matches(
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

fn inactive_startup_failure_authority(
    snapshot: &UnitSnapshot,
    record: &RecordedFailure,
    target: ReconciliationTarget,
) -> Option<StartupFailureRecoveryAuthority> {
    (inactive_systemd_exit_matches(snapshot, record.process_id())
        && record.belongs_to(target.service_generation()))
    .then(|| record.authority(target))
    .flatten()
}

fn inactive_systemd_exit_matches(snapshot: &UnitSnapshot, process_id: u32) -> bool {
    snapshot.active_state == "inactive"
        && snapshot.sub_state == "dead"
        && snapshot.main_process_id == 0
        && snapshot.exec_start_ex.len() == 1
        && snapshot.exec_start_ex.first().is_some_and(|execution| {
            execution.7 == process_id && execution.8 == 1 && execution.9 == 0
        })
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
) -> Result<(), String> {
    manager.recheck_identity()?;
    verify_unit_authority(&manager.unit_path()?, paths, unit)?;
    verify_env_launcher()
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

fn verify_env_launcher() -> Result<(), String> {
    let metadata = fs::symlink_metadata("/usr/bin/env").map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o7777 != 0o755
    {
        return Err("/usr/bin/env must be a root-owned regular executable with mode 0755".into());
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

fn pre_admission(error: impl std::fmt::Display) -> GatewayError {
    GatewayError::Registration(format!("{error}; Nessa made no service or linger change"))
}

fn retire_prior(
    progress: &dyn GatewayReconciliationProgress,
    unit: &SystemdUnitName,
    prior: &ReconciliationIncarnation,
    target: &ReconciliationTarget,
    authority: RetirementAuthority<'_>,
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
    let snapshot = runtime
        .manager
        .snapshot(unit)
        .map_err(GatewayError::Registration)?
        .ok_or_else(|| GatewayError::Registration("The retiring systemd unit vanished".into()))?;
    if snapshot.main_process_id != prior.process_id() {
        return Err(GatewayError::Registration(
            "The retiring systemd snapshot does not identify the planned process".into(),
        ));
    }
    let process = authority
        .process_factory
        .open(snapshot.main_process_id)
        .map_err(|error| {
            GatewayError::Registration(format!(
                "The retiring gateway process cannot be held: {error}"
            ))
        })?;
    let health = runtime
        .context
        .observe_endpoint_health(authority.data)
        .map_err(GatewayError::Registration)?;
    let advertisement = health.as_ref().map(LinuxEndpointHealth::advertisement);
    let retained_path = snapshot
        .exec_start_ex
        .first()
        .and_then(|entry| environment_value(&entry.1, "NESSA_AGENT_PATH"))
        .ok_or_else(|| GatewayError::Registration("The retiring unit has no agent path".into()))
        .and_then(|path| {
            SearchPath::parse(path).map_err(|error| GatewayError::Registration(error.to_string()))
        })?;
    let prior_rendered = render(UnitDefinition {
        unit,
        runtime: &authority
            .paths
            .runtime_root
            .join(prior.target().runtime_fingerprint()),
        configuration: authority.configuration,
        data: authority.data,
        home: authority.home,
        agent_path: &retained_path,
        fingerprint: prior.target().runtime_fingerprint(),
        generation: prior.target().service_generation(),
    })
    .map_err(GatewayError::Registration)?;
    if !snapshot_matches(
        &snapshot,
        runtime.manager,
        authority.paths,
        unit,
        &prior_rendered,
    )? {
        return Err(GatewayError::Registration(
            "The retiring systemd definition changed after planning".into(),
        ));
    }
    if portable_incarnation(unit, Some(&snapshot), advertisement)?.as_ref() != Some(prior)
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
    let result = retire(
        authority.data,
        &request_id,
        prior,
        target,
        process,
        runtime.clock,
        || {
            let refreshed_health = runtime.context.observe_endpoint_health(authority.data)?;
            require_same_endpoint_health(health.as_ref(), refreshed_health.as_ref())?;
            verify_systemd_authority(runtime.manager, authority.paths, unit)?;
            let second = runtime
                .manager
                .snapshot(unit)?
                .ok_or_else(|| "The retiring systemd unit vanished during proof".to_string())?;
            if second != snapshot {
                return Err(
                    "The retiring systemd process changed during exact signal proof".into(),
                );
            }
            Ok(())
        },
    );
    let completion = match &result {
        Ok(()) => LifecycleCommandResult::Accepted,
        Err(error) => LifecycleCommandResult::Failed(error.clone()),
    };
    let observed = observe_systemd_state(runtime.manager, unit, authority.data, runtime.context);
    let artifact_present = observed
        .as_ref()
        .map_err(|error| error.clone())
        .and_then(|observed| {
            retirement_artifact_present(
                authority.data,
                &request_id,
                prior,
                target,
                observed.incarnation.as_ref(),
            )
        });
    if let Err(audit) =
        progress.effect_completed("request-systemd-retirement", step.id(), &completion)
    {
        return Err(audit_with_physical(
            audit,
            result
                .as_ref()
                .err()
                .into_iter()
                .chain(observed.as_ref().err())
                .chain(artifact_present.as_ref().err())
                .cloned(),
        ));
    }
    let observed = observed.map_err(GatewayError::Registration)?;
    let artifact_present = artifact_present.map_err(GatewayError::Registration)?;
    if let Err(audit) = observed.record(
        progress,
        &LifecycleObservationSource::Effect {
            plan_id: "request-systemd-retirement".into(),
            step_id: step.id().into(),
        },
        artifact_present,
    ) {
        return Err(audit_with_physical(
            audit,
            result.as_ref().err().into_iter().cloned(),
        ));
    }
    result.map_err(GatewayError::Registration)
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
    process: Box<dyn LinuxProcess>,
    clock: &dyn MonotonicClock,
    verify_before_signal: impl FnOnce() -> Result<(), String>,
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
    verify_before_signal()?;
    if !process.is_live() {
        return Err("The retiring gateway process exited during exact signal proof".into());
    }
    match process.signal(libc::SIGUSR2) {
        Ok(()) => {}
        Err(LinuxProcessSignalError::Exited) => {
            return Err("The retiring gateway process exited before signal dispatch".into())
        }
        Err(LinuxProcessSignalError::Failed(error)) => return Err(error),
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
            testing::discard_reconciliation_audit, GatewayLifecycleRecoveryAuthority,
            GatewayReconciliationRequest, SystemMonotonicClock,
        },
        domain::value_objects::{
            LifecycleRecordKind, ReconciliationCorrelation, ReconciliationEvidence,
            ReconciliationInitiator, SystemdInvocationId, SystemdManagerIdentity,
        },
        infrastructure::startup_failure::parse_record,
    };
    use nessa_gateway_endpoint::domain::{
        EndpointIdentity, GatewayEndpoint, ManagedRuntimeIdentity,
    };
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn private_tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(".nessa-linux-reconciliation-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("the repository checkout provides a trusted test ancestry")
    }

    #[derive(Clone)]
    struct FixedManagerFactory {
        identity: SystemdManagerIdentity,
        unit_path: Vec<String>,
        snapshot: Option<UnitSnapshot>,
        error: Option<String>,
    }

    impl LinuxManagerFactory for FixedManagerFactory {
        fn verify_prerequisites(&self, _: u32) -> Result<(), String> {
            self.error
                .as_ref()
                .map_or(Ok(()), |error| Err(error.clone()))
        }

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
        ) -> Result<Box<dyn LinuxSystemdJob>, String> {
            unreachable!("recovery must not enqueue systemd work")
        }
    }

    struct FixedRuntimeContext {
        effective_uid: u32,
        real_uid: u32,
        config_home: Option<OsString>,
        data_home: Option<OsString>,
        state_home: Option<OsString>,
        advertisement: Option<GatewayEndpointAdvertisement>,
        endpoint_error: Option<String>,
    }

    impl LinuxRuntimeContext for FixedRuntimeContext {
        fn user_ids(&self) -> (u32, u32) {
            (self.effective_uid, self.real_uid)
        }

        fn xdg_paths(&self) -> (Option<OsString>, Option<OsString>, Option<OsString>) {
            (
                self.config_home.clone(),
                self.data_home.clone(),
                self.state_home.clone(),
            )
        }

        fn observe_endpoint_health(&self, _: &Path) -> Result<Option<LinuxEndpointHealth>, String> {
            self.endpoint_error.as_ref().map_or_else(
                || {
                    Ok(self
                        .advertisement
                        .clone()
                        .map(|advertisement| LinuxEndpointHealth { advertisement }))
                },
                |error| Err(error.clone()),
            )
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

    fn startup_failure_record(generation: &str, process_id: u32) -> RecordedFailure {
        parse_record(
            serde_json::json!({
                "reason": "configuration",
                "exitCode": 20,
                "message": "invalid configuration",
                "serviceGeneration": generation,
                "processId": process_id,
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn inactive_startup_failure_requires_exact_systemd_exit_correlation() {
        let (_, target, mut snapshot) = fixture();
        snapshot.active_state = "inactive".into();
        snapshot.sub_state = "dead".into();
        snapshot.invocation = None;
        snapshot.main_process_id = 0;
        snapshot.exec_start_ex[0].7 = 731;
        snapshot.exec_start_ex[0].8 = 1;
        snapshot.exec_start_ex[0].9 = 0;
        let record = startup_failure_record(target.service_generation(), 731);
        assert_eq!(
            inactive_startup_failure_authority(&snapshot, &record, target.clone())
                .unwrap()
                .process_id(),
            731
        );

        let mut contradictions = Vec::new();
        let mut wrong = snapshot.clone();
        wrong.active_state = "failed".into();
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.sub_state = "exited".into();
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.main_process_id = 731;
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.exec_start_ex[0].7 = 732;
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.exec_start_ex[0].8 = 2;
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.exec_start_ex[0].9 = 1;
        contradictions.push(wrong);
        let mut wrong = snapshot.clone();
        wrong.exec_start_ex.push(wrong.exec_start_ex[0].clone());
        contradictions.push(wrong);

        for contradictory in contradictions {
            assert!(
                inactive_startup_failure_authority(&contradictory, &record, target.clone(),)
                    .is_none()
            );
        }
        let other_target = ReconciliationTarget::new(
            target.service().into(),
            target.runtime_fingerprint().into(),
            "c".repeat(64),
        )
        .unwrap();
        assert!(inactive_startup_failure_authority(&snapshot, &record, other_target).is_none());
    }

    #[test]
    fn systemd_job_results_and_physical_state_are_independent() {
        let (unit, target, snapshot) = fixture();
        let incarnation = ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440001".into(),
            snapshot.main_process_id,
            7420,
        )
        .unwrap();
        let active = ObservedSystemdState {
            native: Some(native_from_snapshot(&snapshot, &target, &unit, true).unwrap()),
            snapshot: Some(snapshot.clone()),
            incarnation: Some(incarnation),
            unit_state: SystemdUnitState::Active,
        };
        let mut restarted_snapshot = snapshot.clone();
        restarted_snapshot.invocation = Some(SystemdInvocationId::new(vec![8; 16]).unwrap());
        restarted_snapshot.main_process_id = 100;
        let restarted = ObservedSystemdState {
            native: Some(native_from_snapshot(&restarted_snapshot, &target, &unit, true).unwrap()),
            snapshot: Some(restarted_snapshot),
            incarnation: Some(
                ReconciliationIncarnation::new(
                    target.clone(),
                    "550e8400-e29b-41d4-a716-446655440002".into(),
                    100,
                    7420,
                )
                .unwrap(),
            ),
            unit_state: SystemdUnitState::Active,
        };
        assert!(active
            .lifecycle_observation(1, true)
            .unwrap()
            .systemd()
            .is_some());
        let mut stopped_snapshot = snapshot.clone();
        stopped_snapshot.active_state = "inactive".into();
        stopped_snapshot.sub_state = "dead".into();
        stopped_snapshot.invocation = None;
        stopped_snapshot.main_process_id = 0;
        let inactive = ObservedSystemdState {
            snapshot: Some(stopped_snapshot.clone()),
            incarnation: None,
            native: None,
            unit_state: SystemdUnitState::Inactive,
        };
        let absent = ObservedSystemdState {
            snapshot: None,
            incarnation: None,
            native: None,
            unit_state: SystemdUnitState::Absent,
        };
        for _completion in [
            LifecycleCommandResult::Accepted,
            LifecycleCommandResult::Rejected("rejected".into()),
            LifecycleCommandResult::Indeterminate("lost".into()),
        ] {
            assert!(systemd_job_reached_state(
                SystemdJobOperation::Start,
                &target,
                &active
            ));
            assert!(!systemd_job_reached_state(
                SystemdJobOperation::Start,
                &target,
                &inactive
            ));
            assert!(systemd_job_reached_state(
                SystemdJobOperation::Stop,
                &target,
                &inactive
            ));
            assert!(systemd_job_reached_state(
                SystemdJobOperation::Stop,
                &target,
                &absent
            ));
            assert!(!systemd_job_reached_state(
                SystemdJobOperation::Stop,
                &target,
                &active
            ));
            assert!(systemd_job_reached_state(
                SystemdJobOperation::Start,
                &target,
                &restarted
            ));
            assert!(!systemd_job_reached_state(
                SystemdJobOperation::Stop,
                &target,
                &restarted
            ));
        }
        assert!(systemd_target_artifact_present(
            &snapshot.manager,
            &unit,
            &target,
            Some(&snapshot),
        )
        .unwrap());
        assert!(systemd_target_artifact_present(
            &stopped_snapshot.manager,
            &unit,
            &target,
            Some(&stopped_snapshot),
        )
        .unwrap());
        let mut wrong_target = stopped_snapshot;
        wrong_target.exec_start_ex[0].1[2] =
            format!("NESSA_RUNTIME_FINGERPRINT={}", "c".repeat(64));
        assert!(!systemd_target_artifact_present(
            &snapshot.manager,
            &unit,
            &target,
            Some(&wrong_target),
        )
        .unwrap());
    }

    #[test]
    fn systemd_job_controller_crosses_every_result_with_every_physical_state() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let (unit, target, base_snapshot) = fixture();
        let paths = LinuxGatewayPaths::new(
            &root.join("home"),
            Some(root.join("config").into_os_string()),
            Some(root.join("data").into_os_string()),
            Some(root.join("state").into_os_string()),
            &unit,
        )
        .unwrap();
        fs::create_dir_all(&paths.unit_root).unwrap();
        fs::set_permissions(&paths.unit_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let unit_path = vec![paths.unit_root.to_string_lossy().into_owned()];
        let effective_uid = unsafe { libc::geteuid() };
        let clock = SystemMonotonicClock;
        let mut serial = 0_u64;

        for (terminal_name, terminal_result) in [
            ("accepted", Ok("done".to_owned())),
            ("rejected", Ok("failed".to_owned())),
            ("indeterminate", Err("job reply lost".to_owned())),
        ] {
            for physical_name in ["active", "inactive", "absent", "restarted"] {
                let mut snapshot = base_snapshot.clone();
                snapshot.unit_path = unit_path.clone();
                let (snapshot, advertisement, running) = match physical_name {
                    "active" => (
                        Some(snapshot),
                        Some(endpoint_for(
                            &target,
                            "550e8400-e29b-41d4-a716-446655440001",
                            99,
                        )),
                        true,
                    ),
                    "restarted" => {
                        snapshot.invocation = Some(SystemdInvocationId::new(vec![8; 16]).unwrap());
                        snapshot.main_process_id = 100;
                        (
                            Some(snapshot),
                            Some(endpoint_for(
                                &target,
                                "550e8400-e29b-41d4-a716-446655440002",
                                100,
                            )),
                            true,
                        )
                    }
                    "inactive" => {
                        snapshot.active_state = "inactive".into();
                        snapshot.sub_state = "dead".into();
                        snapshot.invocation = None;
                        snapshot.main_process_id = 0;
                        (Some(snapshot), None, false)
                    }
                    "absent" => (None, None, false),
                    _ => unreachable!(),
                };
                for operation in [SystemdJobOperation::Start, SystemdJobOperation::Stop] {
                    serial += 1;
                    let correlation = ReconciliationCorrelation::parse(format!(
                        "00000000-0000-4000-8000-{:012x}",
                        serial
                    ))
                    .unwrap();
                    let progress = RecordingProgress {
                        correlation,
                        sequence: AtomicUsize::new(0),
                    };
                    let terminal = terminal_result.clone().map(|result| JobTerminal {
                        manager: base_snapshot.manager.clone(),
                        object_path: "/org/freedesktop/systemd1/job/7".into(),
                        job_id: 7,
                        unit: unit.clone(),
                        result,
                    });
                    let manager = JobManager {
                        identity: base_snapshot.manager.clone(),
                        unit_path: unit_path.clone(),
                        snapshot: snapshot.clone(),
                        terminal,
                    };
                    let context = FixedRuntimeContext {
                        effective_uid,
                        real_uid: effective_uid,
                        config_home: None,
                        data_home: None,
                        state_home: None,
                        advertisement: advertisement.clone(),
                        endpoint_error: None,
                    };
                    let effect = match operation {
                        SystemdJobOperation::Start => LifecycleEffect::StartSystemdUnit {
                            manager: manager.identity.clone(),
                            unit: unit.clone(),
                            mode: SystemdJobMode::Fail,
                        },
                        SystemdJobOperation::Stop => LifecycleEffect::StopSystemdUnit {
                            manager: manager.identity.clone(),
                            unit: unit.clone(),
                            mode: SystemdJobMode::Fail,
                        },
                    };
                    let result = run_systemd_job(
                        &progress,
                        "job-matrix",
                        effect,
                        SystemdJobAuthority {
                            manager: &manager,
                            unit: &unit,
                            target: &target,
                            operation,
                            data: &paths.data_root,
                            paths: &paths,
                            runtime_context: &context,
                            clock: &clock,
                        },
                    );
                    assert_eq!(
                        result.is_ok(),
                        match operation {
                            SystemdJobOperation::Start => running,
                            SystemdJobOperation::Stop => !running,
                        },
                        "{terminal_name}/{physical_name}/{operation:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn audit_failure_after_enqueue_still_reaches_fresh_physical_observation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let (unit, target, mut snapshot) = fixture();
        let paths = LinuxGatewayPaths::new(
            &root.join("home"),
            Some(root.join("config").into_os_string()),
            Some(root.join("data").into_os_string()),
            Some(root.join("state").into_os_string()),
            &unit,
        )
        .unwrap();
        fs::create_dir_all(&paths.unit_root).unwrap();
        fs::set_permissions(&paths.unit_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        snapshot.unit_path = vec![paths.unit_root.to_string_lossy().into_owned()];
        snapshot.active_state = "inactive".into();
        snapshot.sub_state = "dead".into();
        snapshot.invocation = None;
        snapshot.main_process_id = 0;
        let manager = JobManager {
            identity: snapshot.manager.clone(),
            unit_path: snapshot.unit_path.clone(),
            snapshot: Some(snapshot.clone()),
            terminal: Ok(JobTerminal {
                manager: snapshot.manager.clone(),
                object_path: "/org/freedesktop/systemd1/job/7".into(),
                job_id: 7,
                unit: unit.clone(),
                result: "done".into(),
            }),
        };
        let observed = AtomicUsize::new(0);
        let progress = FailingAttemptProgress {
            inner: RecordingProgress {
                correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000999".into(),
                )
                .unwrap(),
                sequence: AtomicUsize::new(0),
            },
            observed: &observed,
        };
        let context = FixedRuntimeContext {
            effective_uid: unsafe { libc::geteuid() },
            real_uid: unsafe { libc::geteuid() },
            config_home: None,
            data_home: None,
            state_home: None,
            advertisement: None,
            endpoint_error: None,
        };
        let result = run_systemd_job(
            &progress,
            "audit-failure",
            LifecycleEffect::StopSystemdUnit {
                manager: manager.identity.clone(),
                unit: unit.clone(),
                mode: SystemdJobMode::Fail,
            },
            SystemdJobAuthority {
                manager: &manager,
                unit: &unit,
                target: &target,
                operation: SystemdJobOperation::Stop,
                data: &paths.data_root,
                paths: &paths,
                runtime_context: &context,
                clock: &SystemMonotonicClock,
            },
        );
        assert!(matches!(result, Err(GatewayError::Audit { .. })));
        assert_eq!(observed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn publication_cleanup_is_predeclared_and_runs_after_completion_delivery_failure() {
        let (_, target, _) = fixture();
        let progress = FailingCompletionProgress {
            planned_cleanup: AtomicUsize::new(0),
            physical_cleanup: AtomicUsize::new(0),
            physical_observations: AtomicUsize::new(0),
        };
        let result = planned_physical_with_cleanup(
            &progress,
            "publication",
            PhysicalPlanWithCleanup {
                primary: LifecycleEffect::PublishSystemdWantsLink {
                    target: target.clone(),
                },
                cleanup: LifecycleEffect::SettleSystemdWantsLinkTransaction { target },
            },
            PhysicalActionsWithCleanup {
                authorize: || Ok(()),
                run: || Ok(()),
                present: || {
                    progress
                        .physical_observations
                        .fetch_add(1, Ordering::SeqCst);
                    Ok(true)
                },
                cleanup_run: |_| {
                    progress.physical_cleanup.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                cleanup_present: || {
                    progress
                        .physical_observations
                        .fetch_add(1, Ordering::SeqCst);
                    Ok(false)
                },
            },
        );
        assert!(result.is_err());
        assert_eq!(progress.planned_cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(progress.physical_cleanup.load(Ordering::SeqCst), 1);
        assert_eq!(progress.physical_observations.load(Ordering::SeqCst), 2);
    }

    struct OwnerChangingManager {
        identity: SystemdManagerIdentity,
        checks: AtomicUsize,
    }

    struct CompletedJob {
        attempt: SystemdJobAttempt,
        terminal: Result<JobTerminal, String>,
    }

    struct RecordingProgress {
        correlation: ReconciliationCorrelation,
        sequence: AtomicUsize,
    }

    struct FailingAttemptProgress<'a> {
        inner: RecordingProgress,
        observed: &'a AtomicUsize,
    }

    struct FailingCompletionProgress {
        planned_cleanup: AtomicUsize,
        physical_cleanup: AtomicUsize,
        physical_observations: AtomicUsize,
    }

    impl GatewayReconciliationProgress for FailingCompletionProgress {
        fn readiness_invalidated(&self) {}
        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }
        fn history_observed(&self, _: ReconciliationHistoryFact) {}
        fn effect_planned(
            &self,
            _: &str,
            _: &LifecyclePlanStep,
            cleanup: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.planned_cleanup.store(cleanup.len(), Ordering::SeqCst);
            Ok(AuditDeliveryReceipt::new(
                ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000778".into())
                    .unwrap(),
                1,
                LifecycleRecordKind::EffectPlan,
            ))
        }
        fn effect_completed(
            &self,
            _: &str,
            _: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Err(GatewayError::Audit {
                audit: "injected completion delivery failure".into(),
                physical: None,
            })
        }
        fn physical_observed(
            &self,
            _: &LifecycleObservationSource,
            _: Option<ReconciliationIncarnation>,
            _: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            unreachable!("completion failure returns after physical cleanup")
        }
    }

    impl GatewayReconciliationProgress for FailingAttemptProgress<'_> {
        fn readiness_invalidated(&self) {}
        fn intent_admitted(&self, intent: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            self.inner.intent_admitted(intent)
        }
        fn history_observed(&self, fact: ReconciliationHistoryFact) {
            self.inner.history_observed(fact)
        }
        fn effect_planned(
            &self,
            plan_id: &str,
            primary: &LifecyclePlanStep,
            cleanup: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.inner.effect_planned(plan_id, primary, cleanup)
        }
        fn effect_completed(
            &self,
            plan_id: &str,
            step_id: &str,
            result: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            self.inner.effect_completed(plan_id, step_id, result)
        }
        fn native_attempt_recorded(
            &self,
            _: &str,
            _: &str,
            _: &SystemdJobAttempt,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Err(GatewayError::Audit {
                audit: "injected native-attempt delivery failure".into(),
                physical: None,
            })
        }
        fn physical_observed(
            &self,
            source: &LifecycleObservationSource,
            incarnation: Option<ReconciliationIncarnation>,
            artifact: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            self.observed.fetch_add(1, Ordering::SeqCst);
            self.inner.physical_observed(source, incarnation, artifact)
        }
        fn systemd_observed(
            &self,
            source: &LifecycleObservationSource,
            incarnation: ReconciliationIncarnation,
            artifact: bool,
            native: SystemdRuntimeObservation,
        ) -> Result<LifecycleObservation, GatewayError> {
            self.observed.fetch_add(1, Ordering::SeqCst);
            self.inner
                .systemd_observed(source, incarnation, artifact, native)
        }
        fn systemd_state_observed(
            &self,
            source: &LifecycleObservationSource,
            artifact: bool,
            state: SystemdUnitState,
        ) -> Result<LifecycleObservation, GatewayError> {
            self.observed.fetch_add(1, Ordering::SeqCst);
            self.inner.systemd_state_observed(source, artifact, state)
        }
    }

    impl RecordingProgress {
        fn next(&self) -> u64 {
            self.sequence.fetch_add(1, Ordering::SeqCst) as u64 + 1
        }

        fn receipt(&self, kind: LifecycleRecordKind) -> AuditDeliveryReceipt {
            AuditDeliveryReceipt::new(self.correlation.clone(), self.next(), kind)
        }
    }

    impl GatewayReconciliationProgress for RecordingProgress {
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
            Ok(self.receipt(LifecycleRecordKind::EffectPlan))
        }

        fn effect_completed(
            &self,
            _: &str,
            _: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(self.receipt(LifecycleRecordKind::EffectCompletion))
        }

        fn native_attempt_recorded(
            &self,
            _: &str,
            _: &str,
            _: &SystemdJobAttempt,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            Ok(self.receipt(LifecycleRecordKind::NativeAttempt))
        }

        fn physical_observed(
            &self,
            _: &LifecycleObservationSource,
            incarnation: Option<ReconciliationIncarnation>,
            target_artifact_present: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            Ok(LifecycleObservation::new(
                self.next(),
                incarnation,
                target_artifact_present,
            ))
        }

        fn systemd_observed(
            &self,
            _: &LifecycleObservationSource,
            incarnation: ReconciliationIncarnation,
            target_artifact_present: bool,
            native: SystemdRuntimeObservation,
        ) -> Result<LifecycleObservation, GatewayError> {
            LifecycleObservation::with_systemd(
                self.next(),
                incarnation,
                target_artifact_present,
                native,
            )
            .map_err(|error| GatewayError::Registration(error.to_string()))
        }

        fn systemd_state_observed(
            &self,
            _: &LifecycleObservationSource,
            target_artifact_present: bool,
            state: SystemdUnitState,
        ) -> Result<LifecycleObservation, GatewayError> {
            LifecycleObservation::with_systemd_state(self.next(), target_artifact_present, state)
                .map_err(|error| GatewayError::Registration(error.to_string()))
        }
    }

    impl LinuxSystemdJob for CompletedJob {
        fn attempt(&self) -> &SystemdJobAttempt {
            &self.attempt
        }

        fn wait(
            self: Box<Self>,
            _: &dyn MonotonicClock,
            _: std::time::Instant,
        ) -> Result<JobTerminal, String> {
            self.terminal
        }
    }

    struct JobManager {
        identity: SystemdManagerIdentity,
        unit_path: Vec<String>,
        snapshot: Option<UnitSnapshot>,
        terminal: Result<JobTerminal, String>,
    }

    impl LinuxUserManager for JobManager {
        fn identity(&self) -> &SystemdManagerIdentity {
            &self.identity
        }

        fn recheck_identity(&self) -> Result<(), String> {
            Ok(())
        }

        fn unit_path(&self) -> Result<Vec<String>, String> {
            Ok(self.unit_path.clone())
        }

        fn reload(&self) -> Result<(), String> {
            unreachable!()
        }

        fn unit_file_state(&self, _: &SystemdUnitName) -> Result<String, String> {
            unreachable!()
        }

        fn get_unit_by_pid(&self, _: u32) -> Result<String, String> {
            unreachable!()
        }

        fn snapshot(&self, _: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String> {
            Ok(self.snapshot.clone())
        }

        fn enqueue(
            &self,
            operation: SystemdJobOperation,
            unit: &SystemdUnitName,
            _: &dyn MonotonicClock,
        ) -> Result<Box<dyn LinuxSystemdJob>, String> {
            Ok(Box::new(CompletedJob {
                attempt: SystemdJobAttempt::new(
                    self.identity.clone(),
                    operation,
                    SystemdJobMode::Fail,
                    unit.clone(),
                    "/org/freedesktop/systemd1/job/7".into(),
                    7,
                )
                .unwrap(),
                terminal: self.terminal.clone(),
            }))
        }
    }

    fn endpoint_for(
        target: &ReconciliationTarget,
        instance: &str,
        process_id: u32,
    ) -> GatewayEndpointAdvertisement {
        let identity = EndpointIdentity::new(instance.into(), process_id).unwrap();
        let endpoint = GatewayEndpoint::new("ws://127.0.0.1:7420".into(), identity).unwrap();
        let managed = ManagedRuntimeIdentity::new(
            target.runtime_fingerprint().into(),
            target.service_generation().into(),
            instance.into(),
            process_id,
        )
        .unwrap();
        GatewayEndpointAdvertisement::new(endpoint, Some(managed)).unwrap()
    }

    #[test]
    fn live_health_is_distinct_from_endpoint_publication_and_must_remain_exact() {
        let (unit, target, snapshot) = fixture();
        let manager = FixedManager {
            identity: snapshot.manager.clone(),
            unit_path: snapshot.unit_path.clone(),
            snapshot: Some(snapshot.clone()),
        };
        let base = FixedRuntimeContext {
            effective_uid: 501,
            real_uid: 501,
            config_home: None,
            data_home: None,
            state_home: None,
            advertisement: None,
            endpoint_error: None,
        };
        let missing = observe_systemd_state(&manager, &unit, Path::new("/data"), &base).unwrap();
        assert!(missing.incarnation.is_none());
        assert_eq!(missing.unit_state, SystemdUnitState::Unknown);

        let unavailable = FixedRuntimeContext {
            endpoint_error: Some("gateway endpoint health deadline elapsed".into()),
            ..base
        };
        assert!(observe_systemd_state(&manager, &unit, Path::new("/data"), &unavailable).is_err());

        let exact = LinuxEndpointHealth {
            advertisement: endpoint_for(&target, "550e8400-e29b-41d4-a716-446655440001", 41),
        };
        let wrong = LinuxEndpointHealth {
            advertisement: endpoint_for(
                &ReconciliationTarget::new(unit.as_str().into(), "c".repeat(64), "d".repeat(64))
                    .unwrap(),
                "550e8400-e29b-41d4-a716-446655440001",
                41,
            ),
        };
        assert!(require_same_endpoint_health(Some(&exact), Some(&exact)).is_ok());
        assert!(require_same_endpoint_health(Some(&exact), Some(&wrong)).is_err());
        assert!(require_same_endpoint_health(Some(&exact), None).is_err());
    }

    impl LinuxUserManager for OwnerChangingManager {
        fn identity(&self) -> &SystemdManagerIdentity {
            &self.identity
        }

        fn recheck_identity(&self) -> Result<(), String> {
            (self.checks.fetch_add(1, Ordering::SeqCst) == 0)
                .then_some(())
                .ok_or_else(|| "manager owner changed".into())
        }

        fn unit_path(&self) -> Result<Vec<String>, String> {
            unreachable!()
        }

        fn reload(&self) -> Result<(), String> {
            unreachable!()
        }

        fn unit_file_state(&self, _: &SystemdUnitName) -> Result<String, String> {
            unreachable!()
        }

        fn get_unit_by_pid(&self, _: u32) -> Result<String, String> {
            unreachable!()
        }

        fn snapshot(&self, _: &SystemdUnitName) -> Result<Option<UnitSnapshot>, String> {
            Ok(None)
        }

        fn enqueue(
            &self,
            _: SystemdJobOperation,
            _: &SystemdUnitName,
            _: &dyn MonotonicClock,
        ) -> Result<Box<dyn LinuxSystemdJob>, String> {
            unreachable!()
        }
    }

    #[test]
    fn observation_refuses_a_manager_owner_change_during_collection() {
        let (unit, _, snapshot) = fixture();
        let manager = OwnerChangingManager {
            identity: snapshot.manager,
            checks: AtomicUsize::new(0),
        };
        let context = FixedRuntimeContext {
            effective_uid: 501,
            real_uid: 501,
            config_home: None,
            data_home: None,
            state_home: None,
            advertisement: None,
            endpoint_error: None,
        };
        assert!(observe_systemd_state(&manager, &unit, Path::new("/data"), &context).is_err());
    }

    #[test]
    fn gateway_paths_use_the_injected_desktop_xdg_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let config = root.join("desktop-config");
        let data = root.join("desktop-data");
        let (unit, _, snapshot) = fixture();
        let effective_uid = unsafe { libc::geteuid() };
        let gateway = SystemdGateway {
            configuration: ServiceConfiguration::new(
                "prod".into(),
                data.join("nessa"),
                None,
                7420,
                None,
            )
            .unwrap(),
            home,
            clock: Arc::new(SystemMonotonicClock),
            manager_factory: Arc::new(FixedManagerFactory {
                identity: snapshot.manager,
                unit_path: vec![],
                snapshot: None,
                error: None,
            }),
            runtime_context: Arc::new(FixedRuntimeContext {
                effective_uid,
                real_uid: effective_uid,
                config_home: Some(config.clone().into_os_string()),
                data_home: Some(data.clone().into_os_string()),
                state_home: Some(root.join("state").into_os_string()),
                advertisement: None,
                endpoint_error: None,
            }),
            process_factory: Arc::new(NativeLinuxProcessFactory),
        };

        let paths = gateway.paths(&unit).unwrap();
        assert_eq!(paths.config_root, config);
        assert_eq!(paths.data_root, data);
        assert_eq!(
            paths.state_root,
            root.join("state/nessa/gateway").join(unit.as_str())
        );
    }

    #[test]
    fn pre_admission_refusal_states_that_no_service_or_linger_change_occurred() {
        let temporary = tempfile::tempdir().unwrap();
        let configuration = ServiceConfiguration::new(
            "prod".into(),
            temporary.path().join("data"),
            None,
            7420,
            None,
        )
        .unwrap();
        let (unit, _, snapshot) = fixture();
        let gateway = SystemdGateway {
            configuration,
            home: temporary.path().join("home"),
            clock: Arc::new(SystemMonotonicClock),
            manager_factory: Arc::new(FixedManagerFactory {
                identity: snapshot.manager,
                unit_path: vec![],
                snapshot: None,
                error: None,
            }),
            runtime_context: Arc::new(FixedRuntimeContext {
                effective_uid: 501,
                real_uid: 501,
                config_home: None,
                data_home: None,
                state_home: None,
                advertisement: None,
                endpoint_error: None,
            }),
            process_factory: Arc::new(NativeLinuxProcessFactory),
        };
        let correlation =
            ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000777".into())
                .unwrap();
        let request = GatewayReconciliationRequest::new(
            correlation.clone(),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let attempt = GatewayReconciliationAttempt::new(
            ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000779".into())
                .unwrap(),
            request,
        )
        .unwrap();
        let progress = RecordingProgress {
            correlation: attempt.correlation().clone(),
            sequence: AtomicUsize::new(0),
        };
        let error = gateway
            .register(Path::new("/unused"), "dev", None, &attempt, &progress)
            .unwrap_err();
        assert!(error
            .to_string()
            .ends_with("Nessa made no service or linger change"));
        assert_eq!(unit.as_str(), "nessa-gateway-prod.service");
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
        let recovery = GatewayLifecycleRecovery::new(
            attempt.clone(),
            GatewayLifecycleRecoveryAuthority::new(target, None, None),
            false,
            None,
            None,
            None,
        );
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
                    config_home: Some(config.clone().into_os_string()),
                    data_home: Some(data.clone().into_os_string()),
                    state_home: Some(root.join("state").into_os_string()),
                    advertisement: None,
                    endpoint_error: endpoint_error.map(str::to_owned),
                }),
                process_factory: Arc::new(NativeLinuxProcessFactory),
            };
            assert!(gateway.recover(&recovery, journal.as_ref()).is_err());
        }

        let gateway = SystemdGateway {
            configuration,
            home,
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
                config_home: Some(config.into_os_string()),
                data_home: Some(data.into_os_string()),
                state_home: Some(root.join("state").into_os_string()),
                advertisement: None,
                endpoint_error: None,
            }),
            process_factory: Arc::new(NativeLinuxProcessFactory),
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
        assert!(retirement_artifact_present(&data, request_id, &prior, &target, None).is_err());
    }

    struct FakeHeldProcess {
        process_id: u32,
        live: bool,
        signals: Arc<AtomicUsize>,
    }

    impl LinuxProcess for FakeHeldProcess {
        fn process_id(&self) -> u32 {
            self.process_id
        }

        fn is_live(&self) -> bool {
            self.live
        }

        fn signal(self: Box<Self>, _: libc::c_int) -> Result<(), LinuxProcessSignalError> {
            self.signals.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn retirement_never_signals_after_post_open_snapshot_or_liveness_change() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().canonicalize().unwrap().join("data");
        nessa_local_storage::create_directory(&data).unwrap();
        let (unit, prior_target, _) = fixture();
        let prior = ReconciliationIncarnation::new(
            prior_target,
            "550e8400-e29b-41d4-a716-446655440001".into(),
            99,
            7420,
        )
        .unwrap();
        let target =
            ReconciliationTarget::new(unit.as_str().into(), "c".repeat(64), "d".repeat(64))
                .unwrap();
        for (live, verification) in [
            (true, Err("snapshot B changed")),
            (true, Err("endpoint health changed")),
            (false, Ok(())),
        ] {
            let signals = Arc::new(AtomicUsize::new(0));
            let result = retire(
                &data,
                "550e8400-e29b-41d4-a716-446655440000",
                &prior,
                &target,
                Box::new(FakeHeldProcess {
                    process_id: prior.process_id(),
                    live,
                    signals: signals.clone(),
                }),
                &SystemMonotonicClock,
                || verification.map_err(str::to_owned),
            );
            assert!(result.is_err());
            assert_eq!(signals.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn settled_definition_recovery_requires_the_admitted_digest() {
        let temporary = private_tempdir();
        let root = temporary.path().canonicalize().unwrap();
        let (unit, target, _) = fixture();
        let paths = LinuxGatewayPaths::new(
            &root.join("home"),
            Some(root.join("config").into_os_string()),
            Some(root.join("data").into_os_string()),
            Some(root.join("state").into_os_string()),
            &unit,
        )
        .unwrap();
        super::super::staging::create_owned_directory_chain(&paths.unit_root).unwrap();
        fs::write(&paths.unit_file, b"admitted bytes").unwrap();
        fs::set_permissions(&paths.unit_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            settle_recovered_definition(&paths, &target, &definition_digest(b"other bytes"),)
                .is_err()
        );
        settle_recovered_definition(&paths, &target, &definition_digest(b"admitted bytes"))
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_manager_factory_refuses_an_account_without_linger_authority() {
        let temporary = tempfile::tempdir().unwrap();
        let sentinel = temporary.path().join("must-remain-absent.service");
        assert!(!sentinel.exists());
        let error = NativeLinuxManagerFactory
            .connect(u32::MAX)
            .err()
            .expect("an unknown account cannot satisfy the linger prerequisite");
        assert!(pre_admission(error)
            .to_string()
            .ends_with("Nessa made no service or linger change"));
        assert!(!sentinel.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "run by the Ubuntu disposable user-manager acceptance step"]
    fn native_user_manager_is_required_for_the_linux_acceptance_gate() {
        use std::os::unix::fs::OpenOptionsExt;

        assert_eq!(
            std::env::var("NESSA_SYSTEMD_ACCEPTANCE").as_deref(),
            Ok("1"),
            "the native fixture must be invoked by the explicit disposable-manager gate"
        );
        let effective_uid = unsafe { libc::geteuid() };
        let manager = UserManager::connect(effective_uid)
            .map(|manager| Box::new(manager) as Box<dyn LinuxUserManager>)
            .expect("Linux acceptance requires the disposable user manager to be reachable");
        assert_eq!(manager.identity().user_id(), effective_uid);
        manager.recheck_identity().unwrap();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{effective_uid}")));
        let unit_root = runtime.join("systemd/user");
        assert!(manager
            .unit_path()
            .unwrap()
            .iter()
            .any(|candidate| Path::new(candidate) == unit_root));
        let unit = SystemdUnitName::parse(format!(
            "nessa-gateway-acceptance-{}.service",
            std::process::id()
        ))
        .unwrap();
        let path = unit_root.join(unit.as_str());
        let failing_unit = SystemdUnitName::parse(format!(
            "nessa-gateway-failing-{}.service",
            std::process::id()
        ))
        .unwrap();
        let failing_path = unit_root.join(failing_unit.as_str());
        let startup_failure_unit = SystemdUnitName::parse(format!(
            "nessa-gateway-startup-failure-{}.service",
            std::process::id()
        ))
        .unwrap();
        let startup_failure_path = unit_root.join(startup_failure_unit.as_str());
        let script = runtime.join(format!(
            ".nessa-gateway-acceptance-{}.sh",
            std::process::id()
        ));
        let startup_failure_script = runtime.join(format!(
            ".nessa-gateway-startup-failure-{}.sh",
            std::process::id()
        ));
        let startup_failure_logs = runtime.join(format!(
            ".nessa-gateway-startup-failure-{}",
            std::process::id()
        ));
        fs::create_dir(&startup_failure_logs)
            .expect("disposable startup-failure log directory must be creatable");
        fs::set_permissions(&startup_failure_logs, fs::Permissions::from_mode(0o700)).unwrap();
        let wants = unit_root.join("default.target.wants");
        let wants_preexisting = wants.exists();
        fs::create_dir_all(&wants).expect("disposable systemd wants directory must be creatable");
        let link = wants.join(unit.as_str());
        std::os::unix::fs::symlink(Path::new("..").join(unit.as_str()), &link)
            .expect("disposable systemd wants link must be creatable");
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("..").join(unit.as_str())
        );
        let mut script_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&script)
            .expect("disposable signal fixture must be creatable");
        script_file
            .write_all(b"#!/bin/sh\ntrap ':' USR1\nwhile :; do sleep 1; done\n")
            .unwrap();
        script_file.sync_all().unwrap();
        drop(script_file);
        let mut startup_failure_script_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&startup_failure_script)
            .expect("disposable startup-failure fixture must be creatable");
        startup_failure_script_file
            .write_all(
                format!(
                    "#!/bin/sh\numask 077\nprintf '{{\"reason\":\"configuration\",\"exitCode\":20,\"message\":\"fixture\",\"serviceGeneration\":\"{}\",\"processId\":%s}}' \"$$\" > '{}'\nexit 0\n",
                    "a".repeat(64),
                    startup_failure_logs
                        .join("gateway-startup-failure.json")
                        .display(),
                )
                .as_bytes(),
            )
            .unwrap();
        startup_failure_script_file.sync_all().unwrap();
        drop(startup_failure_script_file);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("disposable systemd acceptance unit must be creatable");
        let mut failing_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&failing_path)
            .expect("disposable failing systemd unit must be creatable");
        let mut startup_failure_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&startup_failure_path)
            .expect("disposable startup-failure systemd unit must be creatable");
        let clock = SystemMonotonicClock;
        let result = (|| -> Result<(), String> {
            file.write_all(
                format!(
                    "[Unit]\nDescription=Nessa disposable lifecycle acceptance fixture\n[Service]\nType=simple\nExecStart={}\n",
                    script.display()
                )
                .as_bytes(),
            )
            .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            drop(file);
            failing_file
                .write_all(
                    b"[Unit]\nDescription=Nessa disposable failing job fixture\n[Service]\nType=oneshot\nExecStart=/usr/bin/false\n",
                )
                .map_err(|error| error.to_string())?;
            failing_file.sync_all().map_err(|error| error.to_string())?;
            drop(failing_file);
            startup_failure_file
                .write_all(
                    format!(
                        "[Unit]\nDescription=Nessa disposable startup-failure fixture\n[Service]\nType=simple\nExecStart={}\nRestart=on-failure\nRestartSec=5s\nTimeoutStopSec=30s\n",
                        startup_failure_script.display()
                    )
                    .as_bytes(),
                )
                .map_err(|error| error.to_string())?;
            startup_failure_file
                .sync_all()
                .map_err(|error| error.to_string())?;
            drop(startup_failure_file);
            let verification = std::process::Command::new("systemd-analyze")
                .args(["--user", "verify"])
                .arg(&path)
                .arg(&failing_path)
                .arg(&startup_failure_path)
                .output()
                .map_err(|error| format!("systemd-analyze could not start: {error}"))?;
            if !verification.status.success() {
                return Err(format!(
                    "systemd-analyze rejected the disposable units: {}",
                    String::from_utf8_lossy(&verification.stderr).trim()
                ));
            }
            manager.reload()?;
            let start = manager.enqueue(SystemdJobOperation::Start, &unit, &clock)?;
            let start_attempt = start.attempt().clone();
            let terminal = start.wait(&clock, clock.now() + JOB_TIMEOUT)?;
            if terminal.result != "done" {
                return Err(format!("disposable StartUnit returned {}", terminal.result));
            }
            let terminal_evidence = SystemdJobTerminal::new(
                terminal.manager.clone(),
                terminal.object_path.clone(),
                terminal.job_id,
                terminal.unit.clone(),
                terminal.result.clone(),
            )
            .map_err(|error| error.to_string())?;
            if start_attempt.classify_terminal(
                manager.identity(),
                SystemdJobOperation::Start,
                SystemdJobMode::Fail,
                &unit,
                &terminal_evidence,
            ) != SystemdJobConclusion::Accepted
            {
                return Err("real StartUnit evidence did not classify as accepted".into());
            }
            let running = manager
                .snapshot(&unit)?
                .ok_or_else(|| "disposable unit disappeared after StartUnit".to_string())?;
            if running.main_process_id == 0 || running.active_state != "active" {
                return Err("disposable unit did not reach an active process".into());
            }
            let process = NativeLinuxProcessFactory.open(running.main_process_id)?;
            if !process.is_live() {
                return Err("pidfd did not retain the disposable main process".into());
            }
            process.signal(libc::SIGUSR1).map_err(|error| match error {
                LinuxProcessSignalError::Exited => {
                    "disposable process exited before the safe pidfd signal".to_string()
                }
                LinuxProcessSignalError::Failed(error) => error,
            })?;
            let after_signal = manager
                .snapshot(&unit)?
                .ok_or_else(|| "disposable unit disappeared after pidfd signal".to_string())?;
            if after_signal.main_process_id != running.main_process_id
                || !NativeLinuxProcessFactory
                    .open(after_signal.main_process_id)?
                    .is_live()
            {
                return Err(
                    "safe pidfd signal did not preserve the exact disposable process".into(),
                );
            }

            let failing = manager.enqueue(SystemdJobOperation::Start, &failing_unit, &clock)?;
            let failing_attempt = failing.attempt().clone();
            let failing_terminal = failing.wait(&clock, clock.now() + JOB_TIMEOUT)?;
            if failing_terminal.result == "done" {
                return Err("failing disposable StartUnit returned an inexact done result".into());
            }
            let failing_evidence = SystemdJobTerminal::new(
                failing_terminal.manager.clone(),
                failing_terminal.object_path.clone(),
                failing_terminal.job_id,
                failing_terminal.unit.clone(),
                failing_terminal.result.clone(),
            )
            .map_err(|error| error.to_string())?;
            if !matches!(
                failing_attempt.classify_terminal(
                    manager.identity(),
                    SystemdJobOperation::Start,
                    SystemdJobMode::Fail,
                    &failing_unit,
                    &failing_evidence,
                ),
                SystemdJobConclusion::Rejected(_)
            ) {
                return Err("real non-done JobRemoved evidence was not rejected".into());
            }

            let startup_failure =
                manager.enqueue(SystemdJobOperation::Start, &startup_failure_unit, &clock)?;
            let startup_failure_terminal =
                startup_failure.wait(&clock, clock.now() + JOB_TIMEOUT)?;
            if startup_failure_terminal.result != "done" {
                return Err(format!(
                    "startup-failure fixture StartUnit returned {}",
                    startup_failure_terminal.result
                ));
            }
            let deadline = clock.now() + JOB_TIMEOUT;
            let stopped = loop {
                let snapshot = manager.snapshot(&startup_failure_unit)?.ok_or_else(|| {
                    "startup-failure fixture disappeared after StartUnit".to_string()
                })?;
                if snapshot.active_state == "inactive" && snapshot.sub_state == "dead" {
                    break snapshot;
                }
                if clock.now() >= deadline {
                    return Err("startup-failure fixture did not become inactive/dead".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let recorded = recorded_failure(&startup_failure_logs)
                .ok_or_else(|| "startup-failure fixture record was invalid".to_string())?;
            let execution = stopped.exec_start_ex.first().ok_or_else(|| {
                "startup-failure fixture lost retained execution status".to_string()
            })?;
            if stopped.main_process_id != 0
                || stopped.exec_start_ex.len() != 1
                || execution.7 != recorded.process_id()
                || execution.8 != 1
                || execution.9 != 0
            {
                return Err(
                    "Type=simple success did not retain exact PID/CLD_EXITED/status evidence"
                        .into(),
                );
            }

            let stop = manager.enqueue(SystemdJobOperation::Stop, &unit, &clock)?;
            let stop_attempt = stop.attempt().clone();
            let terminal = stop.wait(&clock, clock.now() + JOB_TIMEOUT)?;
            if terminal.result != "done" {
                return Err(format!("disposable StopUnit returned {}", terminal.result));
            }
            let terminal_evidence = SystemdJobTerminal::new(
                terminal.manager.clone(),
                terminal.object_path.clone(),
                terminal.job_id,
                terminal.unit.clone(),
                terminal.result.clone(),
            )
            .map_err(|error| error.to_string())?;
            if stop_attempt.classify_terminal(
                manager.identity(),
                SystemdJobOperation::Stop,
                SystemdJobMode::Fail,
                &unit,
                &terminal_evidence,
            ) != SystemdJobConclusion::Accepted
            {
                return Err("real StopUnit evidence did not classify as accepted".into());
            }
            if let Some(stopped) = manager.snapshot(&unit)? {
                if stopped.main_process_id != 0
                    || stopped.active_state != "inactive"
                    || stopped.sub_state != "dead"
                {
                    return Err("disposable unit did not settle as absent or inactive/dead".into());
                }
            }
            let absent = SystemdUnitName::parse(format!(
                "nessa-gateway-absent-{}.service",
                std::process::id()
            ))
            .map_err(|error| error.to_string())?;
            if manager
                .enqueue(SystemdJobOperation::Start, &absent, &clock)
                .is_ok()
            {
                return Err("StartUnit unexpectedly admitted an absent disposable unit".into());
            }
            Ok(())
        })();
        if result.is_err() {
            if let Ok(job) = manager.enqueue(SystemdJobOperation::Stop, &unit, &clock) {
                let _ = job.wait(&clock, clock.now() + JOB_TIMEOUT);
            }
        }
        if let Ok(job) = manager.enqueue(SystemdJobOperation::Stop, &failing_unit, &clock) {
            let _ = job.wait(&clock, clock.now() + JOB_TIMEOUT);
        }
        if let Ok(job) = manager.enqueue(SystemdJobOperation::Stop, &startup_failure_unit, &clock) {
            let _ = job.wait(&clock, clock.now() + JOB_TIMEOUT);
        }
        let link_removal = fs::remove_file(&link);
        let removal = fs::remove_file(&path);
        let failing_removal = fs::remove_file(&failing_path);
        let startup_failure_removal = fs::remove_file(&startup_failure_path);
        let script_removal = fs::remove_file(&script);
        let startup_failure_script_removal = fs::remove_file(&startup_failure_script);
        let startup_failure_record_removal =
            fs::remove_file(startup_failure_logs.join("gateway-startup-failure.json"));
        let startup_failure_logs_removal = fs::remove_dir(&startup_failure_logs);
        let reload = manager.reload();
        link_removal.expect("disposable systemd wants link cleanup must succeed");
        removal.expect("disposable systemd unit cleanup must succeed");
        failing_removal.expect("disposable failing systemd unit cleanup must succeed");
        startup_failure_removal
            .expect("disposable startup-failure systemd unit cleanup must succeed");
        script_removal.expect("disposable signal fixture cleanup must succeed");
        startup_failure_script_removal
            .expect("disposable startup-failure fixture cleanup must succeed");
        startup_failure_record_removal
            .expect("disposable startup-failure record cleanup must succeed");
        startup_failure_logs_removal
            .expect("disposable startup-failure directory cleanup must succeed");
        if !wants_preexisting {
            fs::remove_dir(&wants).expect("disposable empty wants directory cleanup must succeed");
        }
        reload.expect("systemd must acknowledge disposable unit cleanup");
        result.expect("native disposable StartUnit/StopUnit lifecycle must pass");
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
            state_root: temporary.path().join("state"),
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
    fn authority_refuses_every_systemd_drop_in_scope() {
        let temporary = tempfile::tempdir().unwrap();
        let owned = temporary.path().join("owned");
        fs::create_dir(&owned).unwrap();
        fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o700)).unwrap();
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let paths = LinuxGatewayPaths {
            config_root: temporary.path().join("config"),
            data_root: temporary.path().join("data"),
            state_root: temporary.path().join("state"),
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
    }
}
