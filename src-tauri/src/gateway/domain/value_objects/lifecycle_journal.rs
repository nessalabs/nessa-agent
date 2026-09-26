#![cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(
        dead_code,
        reason = "durable lifecycle journals are implemented only by the macOS adapter"
    )
)]

//! Immutable facts and validation for one gateway lifecycle journal.
//!
//! ```text
//! Intent -> EffectPlan -> EffectCompletion -> Observation -> Outcome
//!    \------ no-effect Observation --------------------------/
//! ```
//! Arrows are allowed predecessor relationships. The same validator is used
//! while appending live records and while restoring records after a restart.

use super::{
    ReconciliationCause, ReconciliationCleanupDecision, ReconciliationCorrelation,
    ReconciliationEvidence, ReconciliationIncarnation, ReconciliationInitiator,
    ReconciliationTarget, StartupFailureRecoveryAuthority, SystemdEvidenceError, SystemdJobAttempt,
    SystemdJobMode, SystemdJobOperation, SystemdManagerIdentity, SystemdRuntimeObservation,
    SystemdUnitName, SystemdUnitState,
};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{self, Display, Formatter},
};

/// Stable kind used in deterministic journal record names and delivery receipts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleRecordKind {
    Intent,
    JoinedRequest,
    EffectPlan,
    NativeAttempt,
    EffectCompletion,
    Observation,
    Outcome,
}

impl LifecycleRecordKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::JoinedRequest => "joined-request",
            Self::EffectPlan => "effect-plan",
            Self::NativeAttempt => "native-attempt",
            Self::EffectCompletion => "effect-completion",
            Self::Observation => "observation",
            Self::Outcome => "outcome",
        }
    }
}

/// Receipt returned only after the final record has been durably acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditDeliveryReceipt {
    attempt_correlation: ReconciliationCorrelation,
    sequence: u64,
    record_kind: LifecycleRecordKind,
}

impl AuditDeliveryReceipt {
    pub fn new(
        attempt_correlation: ReconciliationCorrelation,
        sequence: u64,
        record_kind: LifecycleRecordKind,
    ) -> Self {
        Self {
            attempt_correlation,
            sequence,
            record_kind,
        }
    }

    pub fn attempt_correlation(&self) -> &ReconciliationCorrelation {
        &self.attempt_correlation
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn record_kind(&self) -> LifecycleRecordKind {
        self.record_kind
    }
}

/// One exact native operation authorized by a write-ahead plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEffect {
    StageRuntime {
        fingerprint: String,
    },
    RequestRetirement {
        incarnation: ReconciliationIncarnation,
    },
    RequestSystemdRetirement {
        incarnation: ReconciliationIncarnation,
        request_id: String,
    },
    UnloadService {
        service: String,
    },
    PublishServiceDefinition {
        target: ReconciliationTarget,
    },
    PublishSystemdServiceDefinition {
        target: ReconciliationTarget,
        definition_digest: String,
    },
    SettleSystemdDefinitionTransaction {
        target: ReconciliationTarget,
        definition_digest: String,
    },
    CreateGatewayDataDirectory {
        target: ReconciliationTarget,
    },
    SettleGatewayDataDirectoryTransaction {
        target: ReconciliationTarget,
        generation: String,
    },
    ReloadSystemdManager {
        manager: SystemdManagerIdentity,
        unit: SystemdUnitName,
    },
    CreateSystemdWantsDirectory {
        target: ReconciliationTarget,
    },
    SettleSystemdWantsDirectoryTransaction {
        target: ReconciliationTarget,
        generation: String,
    },
    PublishSystemdWantsLink {
        target: ReconciliationTarget,
    },
    SettleSystemdWantsLinkTransaction {
        target: ReconciliationTarget,
    },
    StartSystemdUnit {
        manager: SystemdManagerIdentity,
        unit: SystemdUnitName,
        mode: SystemdJobMode,
    },
    StopSystemdUnit {
        manager: SystemdManagerIdentity,
        unit: SystemdUnitName,
        mode: SystemdJobMode,
    },
    BootstrapService {
        target: ReconciliationTarget,
    },
    AdoptReadyIncarnation {
        target: ReconciliationTarget,
    },
    PruneRuntime {
        fingerprint: String,
    },
    RemoveStagingRuntime {
        generation: String,
    },
    StopAgents {
        incarnation: ReconciliationIncarnation,
    },
}

/// Condition under which a preplanned contingency may execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEffectPredicate {
    Always,
    PrimaryReturned,
    PrimaryAccepted,
    ObservationMatches(ReconciliationIncarnation),
}

/// Plan-local operation. IDs are unique within their plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecyclePlanStep {
    id: String,
    effect: LifecycleEffect,
    predicate: LifecycleEffectPredicate,
}

impl LifecyclePlanStep {
    pub fn new(
        id: String,
        effect: LifecycleEffect,
        predicate: LifecycleEffectPredicate,
    ) -> Result<Self, LifecycleJournalError> {
        if id.trim().is_empty() {
            return Err(LifecycleJournalError::InvalidStepId);
        }
        let valid_effect = match &effect {
            LifecycleEffect::StageRuntime { fingerprint }
            | LifecycleEffect::PruneRuntime { fingerprint } => sha256_hex(fingerprint),
            LifecycleEffect::RemoveStagingRuntime { generation } => sha256_hex(generation),
            LifecycleEffect::SettleGatewayDataDirectoryTransaction { generation, .. }
            | LifecycleEffect::SettleSystemdWantsDirectoryTransaction { generation, .. } => {
                sha256_hex(generation)
            }
            LifecycleEffect::RequestSystemdRetirement { request_id, .. } => {
                canonical_uuid(request_id)
            }
            LifecycleEffect::UnloadService { service } => !service.trim().is_empty(),
            _ => true,
        };
        if !valid_effect {
            return Err(LifecycleJournalError::InvalidEffect);
        }
        Ok(Self {
            id,
            effect,
            predicate,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn effect(&self) -> &LifecycleEffect {
        &self.effect
    }

    pub fn predicate(&self) -> &LifecycleEffectPredicate {
        &self.predicate
    }
}

/// Native command result. Acceptance is not a physical observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleCommandResult {
    Accepted,
    Rejected(String),
    Failed(String),
    Indeterminate(String),
}

/// Freshly observed service state at a specific monotonically increasing version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleObservation {
    version: u64,
    incarnation: Option<ReconciliationIncarnation>,
    target_artifact_present: bool,
    systemd: Option<Box<SystemdRuntimeObservation>>,
    systemd_state: Option<SystemdUnitState>,
    retained_target: Option<Box<ReconciliationTarget>>,
}

impl LifecycleObservation {
    pub fn new(
        version: u64,
        incarnation: Option<ReconciliationIncarnation>,
        target_artifact_present: bool,
    ) -> Self {
        Self {
            version,
            incarnation,
            target_artifact_present,
            systemd: None,
            systemd_state: None,
            retained_target: None,
        }
    }

    pub fn with_systemd_state(
        version: u64,
        target_artifact_present: bool,
        state: SystemdUnitState,
    ) -> Result<Self, SystemdEvidenceError> {
        if state == SystemdUnitState::Active {
            return Err(SystemdEvidenceError::ContradictoryRuntime);
        }
        Ok(Self {
            version,
            incarnation: None,
            target_artifact_present,
            systemd: None,
            systemd_state: Some(state),
            retained_target: None,
        })
    }

    pub fn with_systemd(
        version: u64,
        incarnation: ReconciliationIncarnation,
        target_artifact_present: bool,
        systemd: SystemdRuntimeObservation,
    ) -> Result<Self, SystemdEvidenceError> {
        if systemd.target() != incarnation.target()
            || systemd.main_process_id() != incarnation.process_id()
        {
            return Err(SystemdEvidenceError::ContradictoryRuntime);
        }
        Ok(Self {
            version,
            incarnation: Some(incarnation),
            target_artifact_present,
            systemd: Some(Box::new(systemd)),
            systemd_state: Some(SystemdUnitState::Active),
            retained_target: None,
        })
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn incarnation(&self) -> Option<&ReconciliationIncarnation> {
        self.incarnation.as_ref()
    }

    pub fn target_artifact_present(&self) -> bool {
        self.target_artifact_present
    }

    pub fn systemd(&self) -> Option<&SystemdRuntimeObservation> {
        self.systemd.as_deref()
    }

    pub fn systemd_state(&self) -> Option<SystemdUnitState> {
        self.systemd_state
    }

    pub fn retained_target(&self) -> Option<&ReconciliationTarget> {
        self.retained_target.as_deref()
    }

    pub fn with_retained_systemd_target(
        version: u64,
        retained_target: ReconciliationTarget,
    ) -> Self {
        Self {
            version,
            incarnation: None,
            target_artifact_present: false,
            systemd: None,
            systemd_state: Some(SystemdUnitState::Inactive),
            retained_target: Some(Box::new(retained_target)),
        }
    }
}

/// Record referenced by an observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleObservationSource {
    Intent,
    Effect { plan_id: String, step_id: String },
}

/// Phase in which a lifecycle stopped making progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleFailedPhase {
    IntentDelivery,
    Planning,
    NativeDispatch,
    NativeCompletionDelivery,
    Observation,
    Cleanup,
    OutcomeDelivery,
}

/// Physical settlement kept separate from delivery acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecyclePhysicalOutcome {
    Confirmed(ReconciliationIncarnation),
    StopAgentsSettled {
        intended: ReconciliationIncarnation,
        command: LifecycleCommandResult,
        observed: LifecycleObservation,
    },
    Failed {
        phase: LifecycleFailedPhase,
        message: String,
    },
}

/// Typed payload of an immutable lifecycle record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleRecordPayload {
    Intent {
        request_correlation: ReconciliationCorrelation,
        cause: ReconciliationCause,
        initiator: ReconciliationInitiator,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
        startup_failure: Option<StartupFailureRecoveryAuthority>,
    },
    JoinedRequest {
        origin_request_correlation: ReconciliationCorrelation,
        request_correlation: ReconciliationCorrelation,
        cause: ReconciliationCause,
        initiator: ReconciliationInitiator,
    },
    EffectPlan {
        plan_id: String,
        expected_before: Option<ReconciliationIncarnation>,
        target: ReconciliationTarget,
        primary: LifecyclePlanStep,
        cleanup: Vec<LifecyclePlanStep>,
    },
    EffectCompletion {
        plan_id: String,
        step_id: String,
        result: LifecycleCommandResult,
    },
    NativeAttempt {
        plan_id: String,
        step_id: String,
        attempt: SystemdJobAttempt,
    },
    Observation {
        source: LifecycleObservationSource,
        state: LifecycleObservation,
    },
    Outcome {
        physical: LifecyclePhysicalOutcome,
        last_confirmed: Option<LifecycleObservation>,
        cleanup: ReconciliationCleanupDecision,
    },
}

impl LifecycleRecordPayload {
    pub fn kind(&self) -> LifecycleRecordKind {
        match self {
            Self::Intent { .. } => LifecycleRecordKind::Intent,
            Self::JoinedRequest { .. } => LifecycleRecordKind::JoinedRequest,
            Self::EffectPlan { .. } => LifecycleRecordKind::EffectPlan,
            Self::NativeAttempt { .. } => LifecycleRecordKind::NativeAttempt,
            Self::EffectCompletion { .. } => LifecycleRecordKind::EffectCompletion,
            Self::Observation { .. } => LifecycleRecordKind::Observation,
            Self::Outcome { .. } => LifecycleRecordKind::Outcome,
        }
    }
}

/// One immutable record in a stage-scoped journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleRecord {
    namespace: String,
    attempt_correlation: ReconciliationCorrelation,
    sequence: u64,
    payload: LifecycleRecordPayload,
}

impl LifecycleRecord {
    pub fn new(
        namespace: String,
        attempt_correlation: ReconciliationCorrelation,
        sequence: u64,
        payload: LifecycleRecordPayload,
    ) -> Result<Self, LifecycleJournalError> {
        if namespace.trim().is_empty() {
            return Err(LifecycleJournalError::InvalidNamespace);
        }
        Ok(Self {
            namespace,
            attempt_correlation,
            sequence,
            payload,
        })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn attempt_correlation(&self) -> &ReconciliationCorrelation {
        &self.attempt_correlation
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn kind(&self) -> LifecycleRecordKind {
        self.payload.kind()
    }

    pub fn payload(&self) -> &LifecycleRecordPayload {
        &self.payload
    }
}

#[derive(Clone, Debug)]
struct PlanState {
    primary_id: String,
    primary_effect: LifecycleEffect,
    steps: BTreeMap<String, LifecyclePlanStep>,
    completed: BTreeMap<String, LifecycleCommandResult>,
    native_attempts: BTreeMap<String, SystemdJobAttempt>,
}

/// The one acknowledged effect boundary that still requires settlement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecyclePendingStep {
    plan_id: String,
    step: LifecyclePlanStep,
    contingencies: Vec<LifecyclePlanStep>,
    completion: Option<LifecycleCommandResult>,
    native_attempt: Option<SystemdJobAttempt>,
}

impl LifecyclePendingStep {
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    pub fn step(&self) -> &LifecyclePlanStep {
        &self.step
    }

    pub fn contingencies(&self) -> &[LifecyclePlanStep] {
        &self.contingencies
    }

    pub fn completion(&self) -> Option<&LifecycleCommandResult> {
        self.completion.as_ref()
    }

    pub fn native_attempt(&self) -> Option<&SystemdJobAttempt> {
        self.native_attempt.as_ref()
    }
}

/// State rebuilt from acknowledged records and advanced by the live writer.
#[derive(Clone, Debug)]
pub struct LifecycleHistory {
    namespace: String,
    attempt_correlation: ReconciliationCorrelation,
    target: ReconciliationTarget,
    before: Option<ReconciliationIncarnation>,
    startup_failure: Option<StartupFailureRecoveryAuthority>,
    next_sequence: u64,
    plans: BTreeMap<String, PlanState>,
    plan_order: Vec<String>,
    pending_observation: Option<(String, String)>,
    latest_observation_source: Option<LifecycleObservationSource>,
    latest_observation: Option<LifecycleObservation>,
    terminal: bool,
    request_correlations: Vec<ReconciliationCorrelation>,
}

impl LifecycleHistory {
    pub fn restore(records: &[LifecycleRecord]) -> Result<Self, LifecycleJournalError> {
        let Some(first) = records.first() else {
            return Err(LifecycleJournalError::MissingIntent);
        };
        let LifecycleRecordPayload::Intent {
            request_correlation,
            cause,
            initiator,
            target,
            before,
            startup_failure,
            ..
        } = first.payload()
        else {
            return Err(LifecycleJournalError::MissingIntent);
        };
        request_correlation
            .require_distinct_from(first.attempt_correlation())
            .map_err(|_| LifecycleJournalError::CorrelationMismatch)?;
        ReconciliationEvidence::new(*cause, *initiator)
            .map_err(|_| LifecycleJournalError::EvidenceMismatch)?;
        if first.namespace() != target.service()
            || before
                .as_ref()
                .is_some_and(|prior| prior.target().service() != first.namespace())
            || startup_failure.as_ref().is_some_and(|authority| {
                before.is_some() || authority.target().service() != first.namespace()
            })
        {
            return Err(LifecycleJournalError::TargetMismatch);
        }
        if first.sequence() != 0 {
            return Err(LifecycleJournalError::SequenceGap);
        }
        let mut history = Self {
            namespace: first.namespace().to_owned(),
            attempt_correlation: first.attempt_correlation().clone(),
            target: target.clone(),
            before: before.clone(),
            startup_failure: startup_failure.clone(),
            next_sequence: 1,
            plans: BTreeMap::new(),
            plan_order: Vec::new(),
            pending_observation: None,
            latest_observation_source: None,
            latest_observation: None,
            terminal: false,
            request_correlations: vec![request_correlation.clone()],
        };
        for record in &records[1..] {
            history.append(record)?;
        }
        Ok(history)
    }

    pub fn append(&mut self, record: &LifecycleRecord) -> Result<(), LifecycleJournalError> {
        if self.terminal {
            return Err(LifecycleJournalError::AfterOutcome);
        }
        if record.namespace() != self.namespace {
            return Err(LifecycleJournalError::NamespaceMismatch);
        }
        if record.attempt_correlation() != &self.attempt_correlation {
            return Err(LifecycleJournalError::CorrelationMismatch);
        }
        if record.sequence() != self.next_sequence {
            return Err(LifecycleJournalError::SequenceGap);
        }
        match record.payload() {
            LifecycleRecordPayload::Intent { .. } => {
                return Err(LifecycleJournalError::SecondIntent)
            }
            LifecycleRecordPayload::JoinedRequest {
                origin_request_correlation,
                request_correlation,
                cause,
                initiator,
            } => {
                if self.request_correlations.first() != Some(origin_request_correlation) {
                    return Err(LifecycleJournalError::CorrelationMismatch);
                }
                request_correlation
                    .require_distinct_from(&self.attempt_correlation)
                    .map_err(|_| LifecycleJournalError::CorrelationMismatch)?;
                ReconciliationEvidence::new(*cause, *initiator)
                    .map_err(|_| LifecycleJournalError::EvidenceMismatch)?;
                if self.request_correlations.contains(request_correlation) {
                    return Err(LifecycleJournalError::DuplicateRequest);
                }
                self.request_correlations.push(request_correlation.clone());
            }
            LifecycleRecordPayload::EffectPlan {
                plan_id,
                expected_before,
                target,
                primary,
                cleanup,
            } => {
                if self.pending_observation.is_some() {
                    return Err(LifecycleJournalError::MissingObservation);
                }
                if self.pending_step().is_some() {
                    return Err(LifecycleJournalError::MissingCompletion);
                }
                if plan_id.trim().is_empty() || self.plans.contains_key(plan_id) {
                    return Err(LifecycleJournalError::InvalidPlan);
                }
                if target != &self.target || expected_before != &self.latest_incarnation() {
                    return Err(LifecycleJournalError::StateMismatch);
                }
                if !matches!(primary.predicate(), LifecycleEffectPredicate::Always) {
                    return Err(LifecycleJournalError::InvalidPlan);
                }
                validate_primary_effect(
                    primary.effect(),
                    expected_before.as_ref(),
                    target,
                    &self.namespace,
                )?;
                let mut steps = BTreeMap::new();
                for step in std::iter::once(primary).chain(cleanup) {
                    if steps.insert(step.id().to_owned(), step.clone()).is_some() {
                        return Err(LifecycleJournalError::DuplicateStep);
                    }
                    if step.id() != primary.id() {
                        validate_cleanup_effect(
                            primary.effect(),
                            step.effect(),
                            target,
                            &self.namespace,
                        )?;
                    }
                }
                self.plans.insert(
                    plan_id.clone(),
                    PlanState {
                        primary_id: primary.id().to_owned(),
                        primary_effect: primary.effect().clone(),
                        steps,
                        completed: BTreeMap::new(),
                        native_attempts: BTreeMap::new(),
                    },
                );
                self.plan_order.push(plan_id.clone());
            }
            LifecycleRecordPayload::NativeAttempt {
                plan_id,
                step_id,
                attempt,
            } => {
                if self.pending_observation.is_some() {
                    return Err(LifecycleJournalError::MissingObservation);
                }
                let plan = self
                    .plans
                    .get_mut(plan_id)
                    .ok_or(LifecycleJournalError::UnknownPlan)?;
                if plan.completed.contains_key(step_id)
                    || plan.native_attempts.contains_key(step_id)
                    || !native_attempt_matches(
                        &plan.primary_effect,
                        step_id,
                        &plan.primary_id,
                        attempt,
                    )
                {
                    return Err(LifecycleJournalError::NativeAttemptMismatch);
                }
                plan.native_attempts
                    .insert(step_id.clone(), attempt.clone());
            }
            LifecycleRecordPayload::EffectCompletion {
                plan_id,
                step_id,
                result,
            } => {
                if self.pending_observation.is_some() {
                    return Err(LifecycleJournalError::MissingObservation);
                }
                let plan = self
                    .plans
                    .get(plan_id)
                    .ok_or(LifecycleJournalError::UnknownPlan)?;
                let predicate = plan
                    .steps
                    .get(step_id)
                    .ok_or(LifecycleJournalError::UnknownOrCompletedStep)?
                    .predicate();
                if plan.completed.contains_key(step_id) {
                    return Err(LifecycleJournalError::UnknownOrCompletedStep);
                }
                if matches!(result, LifecycleCommandResult::Accepted)
                    && matches!(
                        plan.primary_effect,
                        LifecycleEffect::StartSystemdUnit { .. }
                            | LifecycleEffect::StopSystemdUnit { .. }
                    )
                    && !plan.native_attempts.contains_key(step_id)
                {
                    return Err(LifecycleJournalError::NativeAttemptMismatch);
                }
                let allowed = if step_id == &plan.primary_id {
                    true
                } else {
                    match predicate {
                        LifecycleEffectPredicate::Always => true,
                        LifecycleEffectPredicate::PrimaryReturned => {
                            plan.completed.contains_key(&plan.primary_id)
                        }
                        LifecycleEffectPredicate::PrimaryAccepted => matches!(
                            plan.completed.get(&plan.primary_id),
                            Some(LifecycleCommandResult::Accepted)
                        ),
                        LifecycleEffectPredicate::ObservationMatches(expected) => {
                            self.latest_observation
                                .as_ref()
                                .and_then(LifecycleObservation::incarnation)
                                == Some(expected)
                        }
                    }
                };
                if !allowed {
                    return Err(LifecycleJournalError::PredicateNotSatisfied);
                }
                self.plans
                    .get_mut(plan_id)
                    .expect("plan was validated")
                    .completed
                    .insert(step_id.clone(), result.clone());
                self.pending_observation = Some((plan_id.clone(), step_id.clone()));
            }
            LifecycleRecordPayload::Observation { source, state } => {
                if state
                    .incarnation()
                    .is_some_and(|incarnation| incarnation.target().service() != self.namespace)
                {
                    return Err(LifecycleJournalError::TargetMismatch);
                }
                match source {
                    LifecycleObservationSource::Intent if !self.plans.is_empty() => {
                        return Err(LifecycleJournalError::NoEffectProofAfterPlan)
                    }
                    LifecycleObservationSource::Intent => {}
                    LifecycleObservationSource::Effect { plan_id, step_id } => {
                        let plan = self
                            .plans
                            .get(plan_id)
                            .ok_or(LifecycleJournalError::UnknownPlan)?;
                        if !plan.completed.contains_key(step_id) {
                            return Err(LifecycleJournalError::ObservationBeforeCompletion);
                        }
                        if self.pending_observation.as_ref()
                            != Some(&(plan_id.clone(), step_id.clone()))
                        {
                            return Err(LifecycleJournalError::ObservationBeforeCompletion);
                        }
                        self.pending_observation = None;
                    }
                }
                if self
                    .latest_observation
                    .as_ref()
                    .is_some_and(|previous| state.version() <= previous.version())
                {
                    return Err(LifecycleJournalError::ObservationVersionRegression);
                }
                self.latest_observation_source = Some(source.clone());
                self.latest_observation = Some(state.clone());
            }
            LifecycleRecordPayload::Outcome {
                physical,
                last_confirmed,
                cleanup,
            } => {
                if self.pending_observation.is_some() {
                    return Err(LifecycleJournalError::MissingObservation);
                }
                if self.pending_step().is_some() {
                    return Err(LifecycleJournalError::MissingCompletion);
                }
                if last_confirmed != &self.latest_observation {
                    return Err(LifecycleJournalError::StateMismatch);
                }
                match physical {
                    LifecyclePhysicalOutcome::Confirmed(after) => {
                        if *cleanup != ReconciliationCleanupDecision::AdoptClaimed
                            || after.target() != &self.target
                            || self
                                .latest_observation
                                .as_ref()
                                .and_then(LifecycleObservation::incarnation)
                                != Some(after)
                        {
                            return Err(LifecycleJournalError::UnobservedOutcome);
                        }
                    }
                    LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command,
                        observed,
                    } => {
                        let matching_completion = match &self.latest_observation_source {
                            Some(LifecycleObservationSource::Effect { plan_id, step_id }) => {
                                self.plans.get(plan_id).is_some_and(|plan| {
                                    step_id == &plan.primary_id
                                        && matches!(
                                            &plan.primary_effect,
                                            LifecycleEffect::StopAgents { incarnation }
                                                if incarnation == intended
                                        )
                                        && plan.completed.get(step_id) == Some(command)
                                })
                            }
                            Some(LifecycleObservationSource::Intent) | None => false,
                        };
                        if *cleanup != ReconciliationCleanupDecision::RetainPrior
                            || intended != self.before.as_ref().unwrap_or(intended)
                            || Some(observed) != self.latest_observation.as_ref()
                            || !matching_completion
                        {
                            return Err(LifecycleJournalError::StateMismatch);
                        }
                    }
                    LifecyclePhysicalOutcome::Failed { .. }
                        if *cleanup == ReconciliationCleanupDecision::AdoptClaimed =>
                    {
                        return Err(LifecycleJournalError::StateMismatch)
                    }
                    LifecyclePhysicalOutcome::Failed { .. }
                        if *cleanup == ReconciliationCleanupDecision::ClearPrior
                            && self.before.as_ref().is_some_and(|prior| {
                                self.latest_observation
                                    .as_ref()
                                    .and_then(LifecycleObservation::incarnation)
                                    == Some(prior)
                            }) =>
                    {
                        return Err(LifecycleJournalError::StateMismatch)
                    }
                    LifecyclePhysicalOutcome::Failed { .. } if self.plans.is_empty() => {
                        let observation = self
                            .latest_observation
                            .as_ref()
                            .ok_or(LifecycleJournalError::MissingNoEffectObservation)?;
                        let startup_closure =
                            self.startup_failure.as_ref().is_some_and(|authority| {
                                if *cleanup != ReconciliationCleanupDecision::RetainPrior
                                    || observation.incarnation().is_some()
                                {
                                    return false;
                                }
                                match observation.systemd_state() {
                                    Some(SystemdUnitState::Absent) => {
                                        !observation.target_artifact_present()
                                            && observation.retained_target().is_none()
                                    }
                                    Some(SystemdUnitState::Inactive)
                                        if authority.target() == &self.target =>
                                    {
                                        observation.target_artifact_present()
                                            && observation.retained_target().is_none()
                                    }
                                    Some(SystemdUnitState::Inactive) => {
                                        !observation.target_artifact_present()
                                            && observation.retained_target()
                                                == Some(authority.target())
                                    }
                                    _ => false,
                                }
                            });
                        let ordinary_closure = self.startup_failure.is_none()
                            && observation.incarnation() == self.before.as_ref()
                            && !observation.target_artifact_present()
                            && observation.retained_target().is_none();
                        if !startup_closure && !ordinary_closure {
                            return Err(LifecycleJournalError::InvalidNoEffectClosure);
                        }
                    }
                    LifecyclePhysicalOutcome::Failed { .. } => {}
                }
                self.terminal = true;
            }
        }
        self.next_sequence += 1;
        Ok(())
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn latest_observation(&self) -> Option<&LifecycleObservation> {
        self.latest_observation.as_ref()
    }

    pub fn has_effect_plan(&self) -> bool {
        !self.plans.is_empty()
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn startup_failure(&self) -> Option<&StartupFailureRecoveryAuthority> {
        self.startup_failure.as_ref()
    }

    pub fn pending_observation_source(&self) -> Option<LifecycleObservationSource> {
        self.pending_observation.as_ref().map(|(plan_id, step_id)| {
            LifecycleObservationSource::Effect {
                plan_id: plan_id.clone(),
                step_id: step_id.clone(),
            }
        })
    }

    pub fn pending_step(&self) -> Option<LifecyclePendingStep> {
        if let Some((plan_id, step_id)) = &self.pending_observation {
            let plan = self.plans.get(plan_id)?;
            return Some(self.pending(plan_id, plan, step_id));
        }
        for plan_id in &self.plan_order {
            let plan = self.plans.get(plan_id)?;
            if !plan.completed.contains_key(&plan.primary_id) {
                return Some(self.pending(plan_id, plan, &plan.primary_id));
            }
            for (step_id, step) in &plan.steps {
                if step_id == &plan.primary_id || plan.completed.contains_key(step_id) {
                    continue;
                }
                let eligible = match step.predicate() {
                    LifecycleEffectPredicate::Always
                    | LifecycleEffectPredicate::PrimaryReturned => true,
                    LifecycleEffectPredicate::PrimaryAccepted => matches!(
                        plan.completed.get(&plan.primary_id),
                        Some(LifecycleCommandResult::Accepted)
                    ),
                    LifecycleEffectPredicate::ObservationMatches(expected) => {
                        self.latest_observation
                            .as_ref()
                            .and_then(LifecycleObservation::incarnation)
                            == Some(expected)
                    }
                };
                if eligible {
                    return Some(self.pending(plan_id, plan, step_id));
                }
            }
        }
        None
    }

    fn pending(&self, plan_id: &str, plan: &PlanState, step_id: &str) -> LifecyclePendingStep {
        LifecyclePendingStep {
            plan_id: plan_id.into(),
            step: plan.steps[step_id].clone(),
            contingencies: plan
                .steps
                .iter()
                .filter(|(candidate, _)| *candidate != &plan.primary_id)
                .map(|(_, step)| step.clone())
                .collect(),
            completion: plan.completed.get(step_id).cloned(),
            native_attempt: plan.native_attempts.get(step_id).cloned(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn latest_incarnation(&self) -> Option<ReconciliationIncarnation> {
        match &self.latest_observation {
            Some(observation) => observation.incarnation().cloned(),
            None => self.before.clone(),
        }
    }
}

fn validate_primary_effect(
    effect: &LifecycleEffect,
    expected_before: Option<&ReconciliationIncarnation>,
    target: &ReconciliationTarget,
    namespace: &str,
) -> Result<(), LifecycleJournalError> {
    let agrees = match effect {
        LifecycleEffect::StageRuntime { fingerprint } => {
            fingerprint == target.runtime_fingerprint()
        }
        LifecycleEffect::RequestRetirement { incarnation }
        | LifecycleEffect::RequestSystemdRetirement { incarnation, .. }
        | LifecycleEffect::StopAgents { incarnation } => expected_before == Some(incarnation),
        LifecycleEffect::UnloadService { service } => service == namespace,
        LifecycleEffect::PublishServiceDefinition { target: planned }
        | LifecycleEffect::CreateGatewayDataDirectory { target: planned }
        | LifecycleEffect::SettleGatewayDataDirectoryTransaction {
            target: planned, ..
        }
        | LifecycleEffect::CreateSystemdWantsDirectory { target: planned }
        | LifecycleEffect::SettleSystemdWantsDirectoryTransaction {
            target: planned, ..
        }
        | LifecycleEffect::PublishSystemdWantsLink { target: planned }
        | LifecycleEffect::SettleSystemdWantsLinkTransaction { target: planned }
        | LifecycleEffect::BootstrapService { target: planned }
        | LifecycleEffect::AdoptReadyIncarnation { target: planned } => planned == target,
        LifecycleEffect::PublishSystemdServiceDefinition {
            target: planned,
            definition_digest,
        }
        | LifecycleEffect::SettleSystemdDefinitionTransaction {
            target: planned,
            definition_digest,
        } => planned == target && valid_digest(definition_digest),
        LifecycleEffect::ReloadSystemdManager { unit, .. }
        | LifecycleEffect::StartSystemdUnit { unit, .. }
        | LifecycleEffect::StopSystemdUnit { unit, .. } => unit.as_str() == target.service(),
        LifecycleEffect::PruneRuntime { .. } | LifecycleEffect::RemoveStagingRuntime { .. } => true,
    };
    agrees
        .then_some(())
        .ok_or(LifecycleJournalError::TargetMismatch)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn native_attempt_matches(
    effect: &LifecycleEffect,
    step_id: &str,
    primary_id: &str,
    attempt: &SystemdJobAttempt,
) -> bool {
    if step_id != primary_id {
        return false;
    }
    match effect {
        LifecycleEffect::StartSystemdUnit {
            manager,
            unit,
            mode,
        } => {
            attempt.manager() == manager
                && attempt.unit() == unit
                && attempt.mode() == *mode
                && attempt.operation() == SystemdJobOperation::Start
        }
        LifecycleEffect::StopSystemdUnit {
            manager,
            unit,
            mode,
        } => {
            attempt.manager() == manager
                && attempt.unit() == unit
                && attempt.mode() == *mode
                && attempt.operation() == SystemdJobOperation::Stop
        }
        _ => false,
    }
}

fn validate_cleanup_effect(
    primary: &LifecycleEffect,
    cleanup: &LifecycleEffect,
    target: &ReconciliationTarget,
    namespace: &str,
) -> Result<(), LifecycleJournalError> {
    let agrees = match (primary, cleanup) {
        (
            LifecycleEffect::StageRuntime {
                fingerprint: staged,
            },
            LifecycleEffect::PruneRuntime {
                fingerprint: removed,
            },
        ) => staged == removed && staged == target.runtime_fingerprint(),
        (
            LifecycleEffect::StageRuntime { fingerprint },
            LifecycleEffect::RemoveStagingRuntime { generation },
        ) => fingerprint == target.runtime_fingerprint() && sha256_hex(generation),
        (
            LifecycleEffect::BootstrapService { target: planned },
            LifecycleEffect::UnloadService { service },
        ) => planned == target && service == namespace,
        (
            LifecycleEffect::CreateGatewayDataDirectory { target: created },
            LifecycleEffect::SettleGatewayDataDirectoryTransaction {
                target: settled,
                generation,
            },
        )
        | (
            LifecycleEffect::CreateSystemdWantsDirectory { target: created },
            LifecycleEffect::SettleSystemdWantsDirectoryTransaction {
                target: settled,
                generation,
            },
        ) => created == target && settled == target && valid_digest(generation),
        (
            LifecycleEffect::PublishSystemdServiceDefinition {
                target: published,
                definition_digest: published_digest,
            },
            LifecycleEffect::SettleSystemdDefinitionTransaction {
                target: settled,
                definition_digest: settled_digest,
            },
        ) => {
            published == target
                && settled == target
                && published_digest == settled_digest
                && valid_digest(published_digest)
        }
        (
            LifecycleEffect::PublishSystemdWantsLink { target: published },
            LifecycleEffect::SettleSystemdWantsLinkTransaction { target: settled },
        ) => published == target && settled == target,
        _ => false,
    };
    agrees
        .then_some(())
        .ok_or(LifecycleJournalError::TargetMismatch)
}

/// A journal chain violated its protocol or contradicted another field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleJournalError {
    InvalidNamespace,
    InvalidStepId,
    InvalidEffect,
    MissingIntent,
    SecondIntent,
    SequenceGap,
    NamespaceMismatch,
    CorrelationMismatch,
    EvidenceMismatch,
    DuplicateRequest,
    InvalidPlan,
    DuplicateStep,
    TargetMismatch,
    StateMismatch,
    UnknownPlan,
    UnknownOrCompletedStep,
    NativeAttemptMismatch,
    ObservationBeforeCompletion,
    MissingObservation,
    MissingCompletion,
    PredicateNotSatisfied,
    ObservationVersionRegression,
    NoEffectProofAfterPlan,
    MissingNoEffectObservation,
    InvalidNoEffectClosure,
    UnobservedOutcome,
    AfterOutcome,
}

impl Display for LifecycleJournalError {
    fn fmt(&self, output: &mut Formatter<'_>) -> fmt::Result {
        output.write_str(match self {
            Self::InvalidNamespace => "gateway lifecycle namespace is empty",
            Self::InvalidStepId => "gateway lifecycle plan step ID is empty",
            Self::InvalidEffect => "gateway lifecycle plan contains an invalid effect",
            Self::MissingIntent => "gateway lifecycle history does not begin with intent",
            Self::SecondIntent => "gateway lifecycle history contains a second intent",
            Self::SequenceGap => "gateway lifecycle record sequence is not contiguous",
            Self::NamespaceMismatch => "gateway lifecycle records disagree on namespace",
            Self::CorrelationMismatch => "gateway lifecycle records disagree on correlation",
            Self::EvidenceMismatch => "gateway lifecycle cause and initiator disagree",
            Self::DuplicateRequest => "gateway lifecycle repeats a request correlation",
            Self::InvalidPlan => "gateway lifecycle plan ID is empty or repeated",
            Self::DuplicateStep => "gateway lifecycle plan repeats a step ID",
            Self::TargetMismatch => "gateway lifecycle effect disagrees with its target",
            Self::StateMismatch => "gateway lifecycle record disagrees with confirmed state",
            Self::UnknownPlan => "gateway lifecycle record references an unknown plan",
            Self::UnknownOrCompletedStep => {
                "gateway lifecycle completion references an unknown or completed step"
            }
            Self::NativeAttemptMismatch => {
                "gateway native attempt disagrees with its planned manager, operation, mode, unit, or step"
            }
            Self::ObservationBeforeCompletion => {
                "gateway lifecycle observation precedes effect completion"
            }
            Self::MissingObservation => {
                "gateway lifecycle command completion lacks a fresh observation"
            }
            Self::MissingCompletion => {
                "gateway lifecycle effect plan lacks a primary command completion"
            }
            Self::PredicateNotSatisfied => "gateway lifecycle cleanup predicate is not satisfied",
            Self::ObservationVersionRegression => {
                "gateway lifecycle observation version did not increase"
            }
            Self::NoEffectProofAfterPlan => {
                "gateway lifecycle cannot prove no effect after an effect plan"
            }
            Self::MissingNoEffectObservation => {
                "gateway lifecycle no-effect outcome lacks a fresh observation"
            }
            Self::InvalidNoEffectClosure => {
                "gateway lifecycle no-effect observation contradicts prior state"
            }
            Self::UnobservedOutcome => {
                "gateway lifecycle confirmed outcome lacks matching observation"
            }
            Self::AfterOutcome => "gateway lifecycle record follows terminal outcome",
        })
    }
}

impl Error for LifecycleJournalError {}

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn systemd_manager(serial: u32) -> SystemdManagerIdentity {
        SystemdManagerIdentity::new(format!(":1.{serial}"), serial, 501).unwrap()
    }

    fn systemd_unit() -> SystemdUnitName {
        SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap()
    }

    fn systemd_target() -> ReconciliationTarget {
        ReconciliationTarget::new(
            systemd_unit().as_str().into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap()
    }

    fn systemd_record(sequence: u64, payload: LifecycleRecordPayload) -> LifecycleRecord {
        LifecycleRecord::new(
            systemd_unit().as_str().into(),
            correlation(),
            sequence,
            payload,
        )
        .unwrap()
    }

    fn systemd_intent() -> LifecycleRecord {
        systemd_record(
            0,
            LifecycleRecordPayload::Intent {
                request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000099".into(),
                )
                .unwrap(),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
                target: systemd_target(),
                before: None,
                startup_failure: None,
            },
        )
    }

    fn systemd_plan(manager: SystemdManagerIdentity) -> LifecycleRecord {
        systemd_record(
            1,
            LifecycleRecordPayload::EffectPlan {
                plan_id: "start".into(),
                expected_before: None,
                target: systemd_target(),
                primary: LifecyclePlanStep::new(
                    "primary".into(),
                    LifecycleEffect::StartSystemdUnit {
                        manager,
                        unit: systemd_unit(),
                        mode: SystemdJobMode::Fail,
                    },
                    LifecycleEffectPredicate::Always,
                )
                .unwrap(),
                cleanup: vec![],
            },
        )
    }

    fn correlation() -> ReconciliationCorrelation {
        ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000001".into()).unwrap()
    }

    fn target() -> ReconciliationTarget {
        ReconciliationTarget::new("service".into(), "a".repeat(64), "b".repeat(64)).unwrap()
    }

    fn incarnation(serial: u64) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(
            target(),
            format!("00000000-0000-4000-8000-{serial:012x}"),
            serial as u32,
            7420,
        )
        .unwrap()
    }

    fn incarnation_for(service: &str, serial: u64) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(
            ReconciliationTarget::new(service.into(), "c".repeat(64), "d".repeat(64)).unwrap(),
            format!("00000000-0000-4000-8000-{serial:012x}"),
            serial as u32,
            7420,
        )
        .unwrap()
    }

    fn record(sequence: u64, payload: LifecycleRecordPayload) -> LifecycleRecord {
        LifecycleRecord::new("service".into(), correlation(), sequence, payload).unwrap()
    }

    fn intent(before: Option<ReconciliationIncarnation>) -> LifecycleRecord {
        record(
            0,
            LifecycleRecordPayload::Intent {
                request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000099".into(),
                )
                .unwrap(),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
                target: target(),
                before,
                startup_failure: None,
            },
        )
    }

    fn startup_intent(authority_target: ReconciliationTarget) -> LifecycleRecord {
        record(
            0,
            LifecycleRecordPayload::Intent {
                request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000099".into(),
                )
                .unwrap(),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
                target: target(),
                before: None,
                startup_failure: Some(
                    StartupFailureRecoveryAuthority::new(
                        authority_target,
                        "configuration".into(),
                        20,
                        41,
                    )
                    .unwrap(),
                ),
            },
        )
    }

    #[test]
    fn startup_failure_no_plan_closure_preserves_intended_and_retained_targets() {
        let intended = target();
        let old =
            ReconciliationTarget::new(intended.service().into(), "e".repeat(64), "f".repeat(64))
                .unwrap();
        let cases = [
            (
                startup_intent(intended.clone()),
                LifecycleObservation::with_systemd_state(1, true, SystemdUnitState::Inactive)
                    .unwrap(),
                true,
            ),
            (
                startup_intent(old.clone()),
                LifecycleObservation::with_retained_systemd_target(1, old.clone()),
                true,
            ),
            (
                startup_intent(old.clone()),
                LifecycleObservation::with_systemd_state(1, false, SystemdUnitState::Absent)
                    .unwrap(),
                true,
            ),
            (
                startup_intent(old),
                LifecycleObservation::with_systemd_state(1, true, SystemdUnitState::Inactive)
                    .unwrap(),
                false,
            ),
        ];
        for (intent, observation, accepted) in cases {
            let mut history = LifecycleHistory::restore(&[intent]).unwrap();
            history
                .append(&record(
                    1,
                    LifecycleRecordPayload::Observation {
                        source: LifecycleObservationSource::Intent,
                        state: observation.clone(),
                    },
                ))
                .unwrap();
            let before = history.next_sequence();
            let result = history.append(&record(
                2,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Failed {
                        phase: LifecycleFailedPhase::Observation,
                        message: "closed".into(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ));
            assert_eq!(result.is_ok(), accepted);
            if !accepted {
                assert_eq!(history.next_sequence(), before);
            }
        }
    }

    fn plan(sequence: u64) -> LifecycleRecord {
        record(
            sequence,
            LifecycleRecordPayload::EffectPlan {
                plan_id: "replace".into(),
                expected_before: Some(incarnation(10)),
                target: target(),
                primary: LifecyclePlanStep::new(
                    "primary".into(),
                    LifecycleEffect::UnloadService {
                        service: "service".into(),
                    },
                    LifecycleEffectPredicate::Always,
                )
                .unwrap(),
                cleanup: vec![],
            },
        )
    }

    fn stop_plan(sequence: u64, intended: &ReconciliationIncarnation) -> LifecycleRecord {
        named_stop_plan(sequence, "stop-agents", intended)
    }

    fn named_stop_plan(
        sequence: u64,
        plan_id: &str,
        intended: &ReconciliationIncarnation,
    ) -> LifecycleRecord {
        record(
            sequence,
            LifecycleRecordPayload::EffectPlan {
                plan_id: plan_id.into(),
                expected_before: Some(intended.clone()),
                target: target(),
                primary: LifecyclePlanStep::new(
                    "primary".into(),
                    LifecycleEffect::StopAgents {
                        incarnation: intended.clone(),
                    },
                    LifecycleEffectPredicate::Always,
                )
                .unwrap(),
                cleanup: vec![],
            },
        )
    }

    #[test]
    fn every_nonterminal_crash_prefix_restores_to_the_same_state() {
        let records = [
            intent(Some(incarnation(10))),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "replace".into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(1, None, true),
                },
            ),
        ];
        for length in 1..=records.len() {
            let restored = LifecycleHistory::restore(&records[..length]).unwrap();
            assert_eq!(restored.next_sequence(), length as u64);
            assert!(!restored.is_terminal());
        }
    }

    #[test]
    fn accepted_systemd_completion_requires_the_exact_recorded_job_attempt() {
        let manager = systemd_manager(41);
        let prefix = [systemd_intent(), systemd_plan(manager.clone())];
        let accepted = systemd_record(
            2,
            LifecycleRecordPayload::EffectCompletion {
                plan_id: "start".into(),
                step_id: "primary".into(),
                result: LifecycleCommandResult::Accepted,
            },
        );
        assert_eq!(
            LifecycleHistory::restore(&[prefix[0].clone(), prefix[1].clone(), accepted.clone()])
                .unwrap_err(),
            LifecycleJournalError::NativeAttemptMismatch
        );

        let attempt = SystemdJobAttempt::new(
            manager,
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            systemd_unit(),
            "/org/freedesktop/systemd1/job/7".into(),
            7,
        )
        .unwrap();
        let records = [
            prefix[0].clone(),
            prefix[1].clone(),
            systemd_record(
                2,
                LifecycleRecordPayload::NativeAttempt {
                    plan_id: "start".into(),
                    step_id: "primary".into(),
                    attempt,
                },
            ),
            systemd_record(
                3,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "start".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
        ];
        assert_eq!(
            LifecycleHistory::restore(&records).unwrap().next_sequence(),
            4
        );
    }

    #[test]
    fn every_systemd_enqueue_and_terminal_publication_prefix_restores() {
        let manager = systemd_manager(41);
        let attempt = SystemdJobAttempt::new(
            manager.clone(),
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            systemd_unit(),
            "/org/freedesktop/systemd1/job/7".into(),
            7,
        )
        .unwrap();
        let records = [
            systemd_intent(),
            systemd_plan(manager),
            systemd_record(
                2,
                LifecycleRecordPayload::NativeAttempt {
                    plan_id: "start".into(),
                    step_id: "primary".into(),
                    attempt,
                },
            ),
            systemd_record(
                3,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "start".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            systemd_record(
                4,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "start".into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(1, None, true),
                },
            ),
        ];
        for length in 1..=records.len() {
            let restored = LifecycleHistory::restore(&records[..length]).unwrap();
            assert_eq!(restored.next_sequence(), length as u64);
            assert!(!restored.is_terminal());
        }
    }

    #[test]
    fn restored_systemd_attempt_rejects_manager_operation_unit_and_job_disagreement() {
        let manager = systemd_manager(41);
        let cases = [
            SystemdJobAttempt::new(
                systemd_manager(42),
                SystemdJobOperation::Start,
                SystemdJobMode::Fail,
                systemd_unit(),
                "/org/freedesktop/systemd1/job/7".into(),
                7,
            )
            .unwrap(),
            SystemdJobAttempt::new(
                manager.clone(),
                SystemdJobOperation::Stop,
                SystemdJobMode::Fail,
                systemd_unit(),
                "/org/freedesktop/systemd1/job/7".into(),
                7,
            )
            .unwrap(),
            SystemdJobAttempt::new(
                manager.clone(),
                SystemdJobOperation::Start,
                SystemdJobMode::Fail,
                SystemdUnitName::parse("other.service".into()).unwrap(),
                "/org/freedesktop/systemd1/job/7".into(),
                7,
            )
            .unwrap(),
        ];
        for attempt in cases {
            let records = [
                systemd_intent(),
                systemd_plan(manager.clone()),
                systemd_record(
                    2,
                    LifecycleRecordPayload::NativeAttempt {
                        plan_id: "start".into(),
                        step_id: "primary".into(),
                        attempt,
                    },
                ),
            ];
            assert_eq!(
                LifecycleHistory::restore(&records).unwrap_err(),
                LifecycleJournalError::NativeAttemptMismatch
            );
        }
    }

    #[test]
    fn predeclared_bootstrap_cleanup_has_a_valid_chain_at_every_record_boundary() {
        let bootstrap = LifecyclePlanStep::new(
            "primary".into(),
            LifecycleEffect::BootstrapService { target: target() },
            LifecycleEffectPredicate::Always,
        )
        .unwrap();
        let cleanup = LifecyclePlanStep::new(
            "unload-bootstrapped-service".into(),
            LifecycleEffect::UnloadService {
                service: "service".into(),
            },
            LifecycleEffectPredicate::Always,
        )
        .unwrap();
        let records = [
            intent(Some(incarnation(10))),
            record(
                1,
                LifecycleRecordPayload::EffectPlan {
                    plan_id: "bootstrap-service".into(),
                    expected_before: Some(incarnation(10)),
                    target: target(),
                    primary: bootstrap,
                    cleanup: vec![cleanup],
                },
            ),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "bootstrap-service".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "bootstrap-service".into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(1, Some(incarnation(11)), true),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "bootstrap-service".into(),
                    step_id: "unload-bootstrapped-service".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                5,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "bootstrap-service".into(),
                        step_id: "unload-bootstrapped-service".into(),
                    },
                    state: LifecycleObservation::new(2, None, false),
                },
            ),
        ];

        for length in 1..=records.len() {
            let restored = LifecycleHistory::restore(&records[..length]).unwrap();
            assert_eq!(restored.next_sequence(), length as u64);
            assert!(!restored.is_terminal());
        }
    }

    #[test]
    fn systemd_publication_cleanup_requires_exact_target_and_digest_agreement() {
        let target = systemd_target();
        let digest = "c".repeat(64);
        assert!(validate_cleanup_effect(
            &LifecycleEffect::PublishSystemdServiceDefinition {
                target: target.clone(),
                definition_digest: digest.clone(),
            },
            &LifecycleEffect::SettleSystemdDefinitionTransaction {
                target: target.clone(),
                definition_digest: digest.clone(),
            },
            &target,
            target.service(),
        )
        .is_ok());
        assert!(validate_cleanup_effect(
            &LifecycleEffect::PublishSystemdServiceDefinition {
                target: target.clone(),
                definition_digest: digest,
            },
            &LifecycleEffect::SettleSystemdDefinitionTransaction {
                target: target.clone(),
                definition_digest: "d".repeat(64),
            },
            &target,
            target.service(),
        )
        .is_err());
        assert!(validate_cleanup_effect(
            &LifecycleEffect::PublishSystemdWantsLink {
                target: target.clone(),
            },
            &LifecycleEffect::SettleSystemdWantsLinkTransaction {
                target: target.clone(),
            },
            &target,
            target.service(),
        )
        .is_ok());
        let other =
            ReconciliationTarget::new(target.service().into(), "e".repeat(64), "f".repeat(64))
                .unwrap();
        assert!(validate_cleanup_effect(
            &LifecycleEffect::PublishSystemdWantsLink {
                target: target.clone(),
            },
            &LifecycleEffect::SettleSystemdWantsLinkTransaction { target: other },
            &target,
            target.service(),
        )
        .is_err());
    }

    #[test]
    fn directory_cleanup_requires_the_exact_target_and_a_valid_transaction() {
        let target = systemd_target();
        let cleanup = LifecycleEffect::SettleGatewayDataDirectoryTransaction {
            target: target.clone(),
            generation: "7".repeat(64),
        };
        assert!(validate_cleanup_effect(
            &LifecycleEffect::CreateGatewayDataDirectory {
                target: target.clone(),
            },
            &cleanup,
            &target,
            target.service(),
        )
        .is_ok());
        assert!(LifecyclePlanStep::new(
            "cleanup".into(),
            LifecycleEffect::SettleSystemdWantsDirectoryTransaction {
                target,
                generation: "not-a-digest".into(),
            },
            LifecycleEffectPredicate::PrimaryReturned,
        )
        .is_err());
    }

    #[test]
    fn eligible_cleanup_is_the_domain_owned_pending_step_and_blocks_outcome() {
        let target = systemd_target();
        let records = vec![
            systemd_intent(),
            systemd_record(
                1,
                LifecycleRecordPayload::EffectPlan {
                    plan_id: "publish".into(),
                    expected_before: None,
                    target: target.clone(),
                    primary: LifecyclePlanStep::new(
                        "primary".into(),
                        LifecycleEffect::PublishSystemdWantsLink {
                            target: target.clone(),
                        },
                        LifecycleEffectPredicate::Always,
                    )
                    .unwrap(),
                    cleanup: vec![LifecyclePlanStep::new(
                        "settle".into(),
                        LifecycleEffect::SettleSystemdWantsLinkTransaction { target },
                        LifecycleEffectPredicate::PrimaryReturned,
                    )
                    .unwrap()],
                },
            ),
            systemd_record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "publish".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            systemd_record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "publish".into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(1, None, true),
                },
            ),
        ];
        let mut history = LifecycleHistory::restore(&records).unwrap();
        let pending = history.pending_step().expect("cleanup must remain pending");
        assert_eq!(pending.plan_id(), "publish");
        assert_eq!(pending.step().id(), "settle");
        assert!(pending.completion().is_none());
        let outcome = systemd_record(
            4,
            LifecycleRecordPayload::Outcome {
                physical: LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::Cleanup,
                    message: "cleanup unavailable".into(),
                },
                last_confirmed: Some(LifecycleObservation::new(1, None, true)),
                cleanup: ReconciliationCleanupDecision::RetainPrior,
            },
        );
        assert_eq!(
            history.append(&outcome),
            Err(LifecycleJournalError::MissingCompletion)
        );
        assert!(!history.is_terminal());
    }

    #[test]
    fn effect_completion_is_not_a_physical_observation() {
        let records = vec![
            intent(Some(incarnation(10))),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Confirmed(incarnation(11)),
                    last_confirmed: None,
                    cleanup: ReconciliationCleanupDecision::AdoptClaimed,
                },
            ),
        ];
        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::MissingObservation
        );
    }

    #[test]
    fn failed_outcome_cannot_settle_a_plan_without_its_primary_completion() {
        let records = vec![
            intent(Some(incarnation(10))),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Failed {
                        phase: LifecycleFailedPhase::NativeCompletionDelivery,
                        message: "completion acknowledgement failed".into(),
                    },
                    last_confirmed: None,
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];
        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::MissingCompletion
        );
    }

    #[test]
    fn stop_settlement_requires_an_authorizing_stop_plan() {
        let intended = incarnation(10);
        let observation = LifecycleObservation::new(1, Some(intended.clone()), true);
        let records = vec![
            intent(Some(intended.clone())),
            record(
                1,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Intent,
                    state: observation.clone(),
                },
            ),
            record(
                2,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command: LifecycleCommandResult::Accepted,
                        observed: observation.clone(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];

        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::StateMismatch
        );
    }

    #[test]
    fn stop_settlement_command_must_match_the_primary_completion() {
        let intended = incarnation(10);
        let observation = LifecycleObservation::new(1, Some(intended.clone()), true);
        let records = vec![
            intent(Some(intended.clone())),
            stop_plan(1, &intended),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "stop-agents".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "stop-agents".into(),
                        step_id: "primary".into(),
                    },
                    state: observation.clone(),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command: LifecycleCommandResult::Failed("dispatch failed".into()),
                        observed: observation.clone(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];

        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::StateMismatch
        );
    }

    #[test]
    fn matching_stop_plan_completion_and_observation_can_settle() {
        let intended = incarnation(10);
        let observation = LifecycleObservation::new(1, Some(intended.clone()), true);
        let records = vec![
            intent(Some(intended.clone())),
            stop_plan(1, &intended),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "stop-agents".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "stop-agents".into(),
                        step_id: "primary".into(),
                    },
                    state: observation.clone(),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command: LifecycleCommandResult::Accepted,
                        observed: observation.clone(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];

        assert!(LifecycleHistory::restore(&records).unwrap().is_terminal());
    }

    #[test]
    fn stop_settlement_cannot_mix_an_older_command_with_the_latest_plan_observation() {
        let intended = incarnation(10);
        let first_observation = LifecycleObservation::new(1, Some(intended.clone()), true);
        let latest_observation = LifecycleObservation::new(2, Some(intended.clone()), true);
        let records = vec![
            intent(Some(intended.clone())),
            named_stop_plan(1, "first-stop", &intended),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "first-stop".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "first-stop".into(),
                        step_id: "primary".into(),
                    },
                    state: first_observation,
                },
            ),
            named_stop_plan(4, "second-stop", &intended),
            record(
                5,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "second-stop".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Failed("dispatch failed".into()),
                },
            ),
            record(
                6,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "second-stop".into(),
                        step_id: "primary".into(),
                    },
                    state: latest_observation.clone(),
                },
            ),
            record(
                7,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command: LifecycleCommandResult::Accepted,
                        observed: latest_observation.clone(),
                    },
                    last_confirmed: Some(latest_observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];

        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::StateMismatch
        );
    }

    #[test]
    fn effect_plans_reject_cross_field_identity_contradictions() {
        let wrong_target =
            ReconciliationTarget::new("other-service".into(), "c".repeat(64), "d".repeat(64))
                .unwrap();
        let cases = [
            LifecycleEffect::UnloadService {
                service: "other-service".into(),
            },
            LifecycleEffect::StageRuntime {
                fingerprint: "c".repeat(64),
            },
            LifecycleEffect::RequestRetirement {
                incarnation: ReconciliationIncarnation::new(
                    wrong_target,
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                    10,
                    7420,
                )
                .unwrap(),
            },
        ];
        for effect in cases {
            let plan = record(
                1,
                LifecycleRecordPayload::EffectPlan {
                    plan_id: "contradiction".into(),
                    expected_before: Some(incarnation(10)),
                    target: target(),
                    primary: LifecyclePlanStep::new(
                        "primary".into(),
                        effect,
                        LifecycleEffectPredicate::Always,
                    )
                    .unwrap(),
                    cleanup: vec![],
                },
            );
            assert_eq!(
                LifecycleHistory::restore(&[intent(Some(incarnation(10))), plan]).unwrap_err(),
                LifecycleJournalError::TargetMismatch
            );
        }
    }

    #[test]
    fn systemd_retirement_plan_requires_a_canonical_request_identity() {
        assert_eq!(
            LifecyclePlanStep::new(
                "retire".into(),
                LifecycleEffect::RequestSystemdRetirement {
                    incarnation: incarnation(10),
                    request_id: "not-a-uuid".into(),
                },
                LifecycleEffectPredicate::Always,
            ),
            Err(LifecycleJournalError::InvalidEffect)
        );
    }

    #[test]
    fn terminal_cleanup_must_agree_with_the_physical_result() {
        let records = vec![
            intent(None),
            record(
                1,
                LifecycleRecordPayload::EffectPlan {
                    plan_id: "adopt".into(),
                    expected_before: None,
                    target: target(),
                    primary: LifecyclePlanStep::new(
                        "primary".into(),
                        LifecycleEffect::AdoptReadyIncarnation { target: target() },
                        LifecycleEffectPredicate::Always,
                    )
                    .unwrap(),
                    cleanup: vec![],
                },
            ),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "adopt".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "adopt".into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(1, Some(incarnation(11)), true),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Confirmed(incarnation(11)),
                    last_confirmed: Some(LifecycleObservation::new(1, Some(incarnation(11)), true)),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];
        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::UnobservedOutcome
        );
    }

    #[test]
    fn intent_rejects_a_prior_incarnation_from_another_namespace() {
        assert_eq!(
            LifecycleHistory::restore(&[intent(Some(incarnation_for("other-service", 10)))])
                .unwrap_err(),
            LifecycleJournalError::TargetMismatch
        );
    }

    #[test]
    fn cross_namespace_observation_rejection_does_not_mutate_live_history() {
        let prefix = vec![
            intent(Some(incarnation(10))),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
        ];
        let contradiction = record(
            3,
            LifecycleRecordPayload::Observation {
                source: LifecycleObservationSource::Effect {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                },
                state: LifecycleObservation::new(
                    1,
                    Some(incarnation_for("other-service", 11)),
                    true,
                ),
            },
        );
        let mut restored_records = prefix.clone();
        restored_records.push(contradiction.clone());
        assert_eq!(
            LifecycleHistory::restore(&restored_records).unwrap_err(),
            LifecycleJournalError::TargetMismatch
        );

        let mut live = LifecycleHistory::restore(&prefix).unwrap();
        assert_eq!(
            live.append(&contradiction).unwrap_err(),
            LifecycleJournalError::TargetMismatch
        );
        assert_eq!(live.next_sequence(), 3);
        assert_eq!(live.latest_observation(), None);
        live.append(&record(
            3,
            LifecycleRecordPayload::Observation {
                source: LifecycleObservationSource::Effect {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                },
                state: LifecycleObservation::new(1, None, true),
            },
        ))
        .unwrap();
        assert_eq!(live.next_sequence(), 4);
    }

    #[test]
    fn same_namespace_replacement_remains_valid_stop_observation() {
        let intended = incarnation(10);
        let replacement = incarnation_for("service", 11);
        let observation = LifecycleObservation::new(1, Some(replacement), true);
        let records = vec![
            intent(Some(intended.clone())),
            stop_plan(1, &intended),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "stop-agents".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "stop-agents".into(),
                        step_id: "primary".into(),
                    },
                    state: observation.clone(),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::StopAgentsSettled {
                        intended,
                        command: LifecycleCommandResult::Accepted,
                        observed: observation.clone(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ),
        ];

        assert!(LifecycleHistory::restore(&records).unwrap().is_terminal());
    }

    #[test]
    fn failed_clear_prior_rejection_does_not_mutate_live_history() {
        let prior = incarnation(10);
        let observation = LifecycleObservation::new(1, Some(prior.clone()), false);
        let prefix = vec![
            intent(Some(prior)),
            record(
                1,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Intent,
                    state: observation.clone(),
                },
            ),
        ];
        let contradiction = record(
            2,
            LifecycleRecordPayload::Outcome {
                physical: LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::Planning,
                    message: "planning failed".into(),
                },
                last_confirmed: Some(observation.clone()),
                cleanup: ReconciliationCleanupDecision::ClearPrior,
            },
        );
        let mut restored_records = prefix.clone();
        restored_records.push(contradiction.clone());
        assert_eq!(
            LifecycleHistory::restore(&restored_records).unwrap_err(),
            LifecycleJournalError::StateMismatch
        );

        let mut live = LifecycleHistory::restore(&prefix).unwrap();
        assert_eq!(
            live.append(&contradiction).unwrap_err(),
            LifecycleJournalError::StateMismatch
        );
        assert_eq!(live.next_sequence(), 2);
        assert!(!live.is_terminal());
        live.append(&record(
            2,
            LifecycleRecordPayload::Outcome {
                physical: LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::Planning,
                    message: "planning failed".into(),
                },
                last_confirmed: Some(observation),
                cleanup: ReconciliationCleanupDecision::RetainPrior,
            },
        ))
        .unwrap();
        assert!(live.is_terminal());
    }

    #[test]
    fn failed_outcome_accepts_retain_or_clear_after_confirmed_absence() {
        for cleanup in [
            ReconciliationCleanupDecision::RetainPrior,
            ReconciliationCleanupDecision::ClearPrior,
        ] {
            let observation = LifecycleObservation::new(1, None, false);
            let records = vec![
                intent(None),
                record(
                    1,
                    LifecycleRecordPayload::Observation {
                        source: LifecycleObservationSource::Intent,
                        state: observation.clone(),
                    },
                ),
                record(
                    2,
                    LifecycleRecordPayload::Outcome {
                        physical: LifecyclePhysicalOutcome::Failed {
                            phase: LifecycleFailedPhase::Planning,
                            message: "planning failed".into(),
                        },
                        last_confirmed: Some(observation),
                        cleanup,
                    },
                ),
            ];

            assert!(LifecycleHistory::restore(&records).unwrap().is_terminal());
        }
    }

    #[test]
    fn failed_outcome_can_clear_a_retired_prior_when_a_replacement_is_observed() {
        let prior = incarnation(10);
        let replacement = incarnation_for("service", 11);
        let observation = LifecycleObservation::new(1, Some(replacement), true);
        let records = vec![
            intent(Some(prior)),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: "replace".into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ),
            record(
                3,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: "replace".into(),
                        step_id: "primary".into(),
                    },
                    state: observation.clone(),
                },
            ),
            record(
                4,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Failed {
                        phase: LifecycleFailedPhase::Observation,
                        message: "replacement did not become ready".into(),
                    },
                    last_confirmed: Some(observation),
                    cleanup: ReconciliationCleanupDecision::ClearPrior,
                },
            ),
        ];

        assert!(LifecycleHistory::restore(&records).unwrap().is_terminal());
    }

    #[test]
    fn a_complete_stale_replacement_chain_accepts_old_and_new_identities_in_their_roles() {
        let old_target =
            ReconciliationTarget::new("service".into(), "c".repeat(64), "d".repeat(64)).unwrap();
        let old = ReconciliationIncarnation::new(
            old_target,
            "00000000-0000-4000-8000-000000000010".into(),
            10,
            7420,
        )
        .unwrap();
        let new = incarnation(11);
        let mut records = vec![intent(Some(old.clone()))];
        let operations = [
            (
                "retire",
                LifecycleEffect::RequestRetirement {
                    incarnation: old.clone(),
                },
                Some(old.clone()),
            ),
            (
                "unload",
                LifecycleEffect::UnloadService {
                    service: "service".into(),
                },
                None,
            ),
            (
                "publish",
                LifecycleEffect::PublishServiceDefinition { target: target() },
                None,
            ),
            (
                "bootstrap",
                LifecycleEffect::BootstrapService { target: target() },
                Some(new.clone()),
            ),
            (
                "adopt",
                LifecycleEffect::AdoptReadyIncarnation { target: target() },
                Some(new.clone()),
            ),
        ];
        for (index, (plan_id, effect, observed)) in operations.into_iter().enumerate() {
            let sequence = records.len() as u64;
            let expected_before = records
                .iter()
                .rev()
                .find_map(|record| match record.payload() {
                    LifecycleRecordPayload::Observation { state, .. } => {
                        Some(state.incarnation().cloned())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| Some(old.clone()));
            records.push(record(
                sequence,
                LifecycleRecordPayload::EffectPlan {
                    plan_id: plan_id.into(),
                    expected_before,
                    target: target(),
                    primary: LifecyclePlanStep::new(
                        "primary".into(),
                        effect,
                        LifecycleEffectPredicate::Always,
                    )
                    .unwrap(),
                    cleanup: vec![],
                },
            ));
            records.push(record(
                sequence + 1,
                LifecycleRecordPayload::EffectCompletion {
                    plan_id: plan_id.into(),
                    step_id: "primary".into(),
                    result: LifecycleCommandResult::Accepted,
                },
            ));
            records.push(record(
                sequence + 2,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Effect {
                        plan_id: plan_id.into(),
                        step_id: "primary".into(),
                    },
                    state: LifecycleObservation::new(index as u64 + 1, observed, index >= 2),
                },
            ));
        }
        let final_observation = LifecycleObservation::new(5, Some(new.clone()), true);
        records.push(record(
            records.len() as u64,
            LifecycleRecordPayload::Outcome {
                physical: LifecyclePhysicalOutcome::Confirmed(new),
                last_confirmed: Some(final_observation),
                cleanup: ReconciliationCleanupDecision::AdoptClaimed,
            },
        ));

        assert!(LifecycleHistory::restore(&records).unwrap().is_terminal());
    }

    #[test]
    fn absence_after_a_plan_cannot_prove_no_effect() {
        let records = vec![
            intent(Some(incarnation(10))),
            plan(1),
            record(
                2,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Intent,
                    state: LifecycleObservation::new(1, Some(incarnation(10)), false),
                },
            ),
        ];
        assert_eq!(
            LifecycleHistory::restore(&records).unwrap_err(),
            LifecycleJournalError::NoEffectProofAfterPlan
        );
    }

    #[test]
    fn no_effect_closure_requires_prior_state_and_no_target_artifact() {
        let prior = incarnation(10);
        let mut history = LifecycleHistory::restore(&[intent(Some(prior.clone()))]).unwrap();
        history
            .append(&record(
                1,
                LifecycleRecordPayload::Observation {
                    source: LifecycleObservationSource::Intent,
                    state: LifecycleObservation::new(1, Some(prior.clone()), false),
                },
            ))
            .unwrap();
        history
            .append(&record(
                2,
                LifecycleRecordPayload::Outcome {
                    physical: LifecyclePhysicalOutcome::Failed {
                        phase: LifecycleFailedPhase::Planning,
                        message: "audit unavailable".into(),
                    },
                    last_confirmed: Some(LifecycleObservation::new(1, Some(prior), false)),
                    cleanup: ReconciliationCleanupDecision::RetainPrior,
                },
            ))
            .unwrap();
        assert!(history.is_terminal());
    }

    #[test]
    fn namespace_and_sequence_disagreement_fail_closed() {
        let first = intent(None);
        let wrong_namespace = LifecycleRecord::new(
            "other".into(),
            correlation(),
            1,
            LifecycleRecordPayload::JoinedRequest {
                origin_request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000099".into(),
                )
                .unwrap(),
                request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000002".into(),
                )
                .unwrap(),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
            },
        )
        .unwrap();
        assert_eq!(
            LifecycleHistory::restore(&[first.clone(), wrong_namespace]).unwrap_err(),
            LifecycleJournalError::NamespaceMismatch
        );
        let gap = record(
            2,
            LifecycleRecordPayload::JoinedRequest {
                origin_request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000099".into(),
                )
                .unwrap(),
                request_correlation: ReconciliationCorrelation::parse(
                    "00000000-0000-4000-8000-000000000003".into(),
                )
                .unwrap(),
                cause: ReconciliationCause::Startup,
                initiator: ReconciliationInitiator::DesktopHost,
            },
        );
        assert_eq!(
            LifecycleHistory::restore(&[first, gap]).unwrap_err(),
            LifecycleJournalError::SequenceGap
        );
    }
}
