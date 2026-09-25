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
    ReconciliationTarget,
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
    UnloadService {
        service: String,
    },
    PublishServiceDefinition {
        target: ReconciliationTarget,
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

impl LifecycleEffect {
    pub fn target(&self) -> Option<&ReconciliationTarget> {
        match self {
            Self::RequestRetirement { incarnation } | Self::StopAgents { incarnation } => {
                Some(incarnation.target())
            }
            Self::PublishServiceDefinition { target }
            | Self::BootstrapService { target }
            | Self::AdoptReadyIncarnation { target } => Some(target),
            Self::StageRuntime { .. }
            | Self::UnloadService { .. }
            | Self::PruneRuntime { .. }
            | Self::RemoveStagingRuntime { .. } => None,
        }
    }
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
        }
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
    steps: BTreeMap<String, LifecycleEffectPredicate>,
    completed: BTreeMap<String, LifecycleCommandResult>,
}

/// State rebuilt from acknowledged records and advanced by the live writer.
#[derive(Clone, Debug)]
pub struct LifecycleHistory {
    namespace: String,
    attempt_correlation: ReconciliationCorrelation,
    target: ReconciliationTarget,
    before: Option<ReconciliationIncarnation>,
    next_sequence: u64,
    plans: BTreeMap<String, PlanState>,
    pending_observation: Option<(String, String)>,
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
        if first.namespace() != target.service() {
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
            next_sequence: 1,
            plans: BTreeMap::new(),
            pending_observation: None,
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
                if plan_id.trim().is_empty() || self.plans.contains_key(plan_id) {
                    return Err(LifecycleJournalError::InvalidPlan);
                }
                if target != &self.target || expected_before != &self.latest_incarnation() {
                    return Err(LifecycleJournalError::StateMismatch);
                }
                if !matches!(primary.predicate(), LifecycleEffectPredicate::Always) {
                    return Err(LifecycleJournalError::InvalidPlan);
                }
                let mut steps = BTreeMap::new();
                for step in std::iter::once(primary).chain(cleanup) {
                    if steps
                        .insert(step.id().to_owned(), step.predicate().clone())
                        .is_some()
                    {
                        return Err(LifecycleJournalError::DuplicateStep);
                    }
                    if step
                        .effect()
                        .target()
                        .is_some_and(|target| target != &self.target)
                    {
                        return Err(LifecycleJournalError::TargetMismatch);
                    }
                }
                self.plans.insert(
                    plan_id.clone(),
                    PlanState {
                        primary_id: primary.id().to_owned(),
                        steps,
                        completed: BTreeMap::new(),
                    },
                );
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
                    .ok_or(LifecycleJournalError::UnknownOrCompletedStep)?;
                if plan.completed.contains_key(step_id) {
                    return Err(LifecycleJournalError::UnknownOrCompletedStep);
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
                self.latest_observation = Some(state.clone());
            }
            LifecycleRecordPayload::Outcome {
                physical,
                last_confirmed,
                ..
            } => {
                if self.pending_observation.is_some() {
                    return Err(LifecycleJournalError::MissingObservation);
                }
                if last_confirmed != &self.latest_observation {
                    return Err(LifecycleJournalError::StateMismatch);
                }
                match physical {
                    LifecyclePhysicalOutcome::Confirmed(after) => {
                        if after.target() != &self.target
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
                        intended, observed, ..
                    } => {
                        if intended != self.before.as_ref().unwrap_or(intended)
                            || Some(observed) != self.latest_observation.as_ref()
                        {
                            return Err(LifecycleJournalError::StateMismatch);
                        }
                    }
                    LifecyclePhysicalOutcome::Failed { .. } if self.plans.is_empty() => {
                        let observation = self
                            .latest_observation
                            .as_ref()
                            .ok_or(LifecycleJournalError::MissingNoEffectObservation)?;
                        if observation.incarnation() != self.before.as_ref()
                            || observation.target_artifact_present()
                        {
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
    ObservationBeforeCompletion,
    MissingObservation,
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
            Self::ObservationBeforeCompletion => {
                "gateway lifecycle observation precedes effect completion"
            }
            Self::MissingObservation => {
                "gateway lifecycle command completion lacks a fresh observation"
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

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        )
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
