#![cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(
        dead_code,
        reason = "native reconciliation evidence is exercised only by the macOS adapter"
    )
)]

//! Validated cause, caller, correlation, target, and incarnation evidence for
//! one gateway reconciliation lifecycle.
//!
//! ```text
//! verified command caller -> request -> attempt -> target -> native outcome
//! ```
//! Arrows mean immutable evidence carried forward; constructors reject facts
//! that contradict the protocol before infrastructure may persist them.
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

/// Why the host requested service reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationCause {
    /// Packaged host startup or automatic liveness recovery.
    Startup,
    /// A bundled surface needs a credential backed by a current gateway.
    CredentialLoad,
    /// A person explicitly chose Retry in a bundled surface.
    ExplicitRetry,
    /// A person changed Claude's explicit configuration directory.
    ClaudeConfigurationChanged,
    /// The desktop host is applying the automatic quit policy.
    DesktopQuitPolicy,
}

/// A bundled surface whose native window label was verified by the host seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundledSurface {
    Main,
    Setup,
}

/// Who initiated a reconciliation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationInitiator {
    /// Automatic work owned by the desktop host.
    DesktopHost,
    /// A verified bundled Nessa surface.
    BundledSurface(BundledSurface),
}

/// Immutable request evidence retained across retries and coalescing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationEvidence {
    cause: ReconciliationCause,
    initiator: ReconciliationInitiator,
}

impl ReconciliationEvidence {
    pub fn new(
        cause: ReconciliationCause,
        initiator: ReconciliationInitiator,
    ) -> Result<Self, ReconciliationEvidenceError> {
        let valid = matches!(
            (cause, initiator),
            (
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost
            ) | (
                ReconciliationCause::ClaudeConfigurationChanged,
                ReconciliationInitiator::DesktopHost
            ) | (
                ReconciliationCause::DesktopQuitPolicy,
                ReconciliationInitiator::DesktopHost
            ) | (
                ReconciliationCause::CredentialLoad | ReconciliationCause::ExplicitRetry,
                ReconciliationInitiator::BundledSurface(_),
            ) | (
                ReconciliationCause::ClaudeConfigurationChanged,
                ReconciliationInitiator::BundledSurface(_),
            )
        );
        valid
            .then_some(Self { cause, initiator })
            .ok_or(ReconciliationEvidenceError)
    }

    pub fn cause(&self) -> ReconciliationCause {
        self.cause
    }

    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn initiator(&self) -> ReconciliationInitiator {
        self.initiator
    }
}

/// Cause and initiator contradict one another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconciliationEvidenceError;

impl Display for ReconciliationEvidenceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("gateway reconciliation cause has no valid initiator")
    }
}

impl Error for ReconciliationEvidenceError {}

/// UUID correlation allocated at an injected infrastructure boundary.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ReconciliationCorrelation(String);

impl ReconciliationCorrelation {
    pub fn parse(value: String) -> Result<Self, ReconciliationCorrelationError> {
        if canonical_uuid(&value) {
            Ok(Self(value))
        } else {
            Err(ReconciliationCorrelationError)
        }
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn require_distinct_from(
        &self,
        other: &Self,
    ) -> Result<(), ReconciliationCorrelationPairError> {
        if self == other {
            Err(ReconciliationCorrelationPairError)
        } else {
            Ok(())
        }
    }
}

/// A reconciliation correlation was not a canonical lowercase UUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconciliationCorrelationError;

impl Display for ReconciliationCorrelationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("gateway reconciliation correlation must be a canonical UUID")
    }
}

impl Error for ReconciliationCorrelationError {}

/// Request and attempt correlations must identify different lifecycle facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconciliationCorrelationPairError;

impl Display for ReconciliationCorrelationPairError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("gateway request and attempt correlations must be distinct")
    }
}

impl Error for ReconciliationCorrelationPairError {}

/// Validated identity that a reconciliation attempt is admitted to establish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationTarget {
    service: String,
    runtime_fingerprint: String,
    service_generation: String,
}

/// Exact server evidence that permits retrying an inactive managed registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupFailureRecoveryAuthority {
    target: ReconciliationTarget,
    reason: String,
    exit_code: u8,
    process_id: u32,
}

impl StartupFailureRecoveryAuthority {
    pub(crate) fn new(
        target: ReconciliationTarget,
        reason: String,
        exit_code: u8,
        process_id: u32,
    ) -> Result<Self, ReconciliationConsistencyError> {
        if reason.trim().is_empty() || exit_code == 0 || process_id == 0 {
            return Err(ReconciliationConsistencyError::InvalidStartupFailureAuthority);
        }
        Ok(Self {
            target,
            reason,
            exit_code,
            process_id,
        })
    }
    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}

impl ReconciliationTarget {
    pub fn new(
        service: String,
        runtime_fingerprint: String,
        service_generation: String,
    ) -> Result<Self, ReconciliationIdentityError> {
        if service.trim().is_empty()
            || !sha256_hex(&runtime_fingerprint)
            || !sha256_hex(&service_generation)
        {
            return Err(ReconciliationIdentityError);
        }
        Ok(Self {
            service,
            runtime_fingerprint,
            service_generation,
        })
    }

    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn service(&self) -> &str {
        &self.service
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn service_generation(&self) -> &str {
        &self.service_generation
    }
}

/// Validated native incarnation observed before or after reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationIncarnation {
    target: ReconciliationTarget,
    runtime_instance: String,
    process_id: u32,
    port: u16,
}

impl ReconciliationIncarnation {
    pub fn new(
        target: ReconciliationTarget,
        runtime_instance: String,
        process_id: u32,
        port: u16,
    ) -> Result<Self, ReconciliationIdentityError> {
        if !canonical_uuid(&runtime_instance) || process_id == 0 || port == 0 {
            return Err(ReconciliationIdentityError);
        }
        Ok(Self {
            target,
            runtime_instance,
            process_id,
            port,
        })
    }

    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }

    pub fn runtime_instance(&self) -> &str {
        &self.runtime_instance
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    #[cfg_attr(
        not(any(target_os = "macos", target_os = "linux")),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Reconciliation identity evidence was empty or structurally invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconciliationIdentityError;

impl Display for ReconciliationIdentityError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("gateway reconciliation identity is invalid")
    }
}

impl Error for ReconciliationIdentityError {}

/// Validated request identity and causal evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationRequestRecord {
    correlation: ReconciliationCorrelation,
    evidence: ReconciliationEvidence,
}

impl ReconciliationRequestRecord {
    pub fn new(correlation: ReconciliationCorrelation, evidence: ReconciliationEvidence) -> Self {
        Self {
            correlation,
            evidence,
        }
    }
    pub fn correlation(&self) -> &ReconciliationCorrelation {
        &self.correlation
    }
    pub fn evidence(&self) -> &ReconciliationEvidence {
        &self.evidence
    }
}

/// Validated attempt identity tied to the request that caused it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationAttemptRecord {
    correlation: ReconciliationCorrelation,
    origin: ReconciliationRequestRecord,
}

impl ReconciliationAttemptRecord {
    pub fn new(
        correlation: ReconciliationCorrelation,
        origin: ReconciliationRequestRecord,
    ) -> Result<Self, ReconciliationConsistencyError> {
        correlation
            .require_distinct_from(origin.correlation())
            .map_err(|_| ReconciliationConsistencyError::EqualRequestAndAttempt)?;
        Ok(Self {
            correlation,
            origin,
        })
    }
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "the native audit adapter is supported only on macOS and Linux"
        )
    )]
    pub fn correlation(&self) -> &ReconciliationCorrelation {
        &self.correlation
    }
}

/// Validated intent whose prior incarnation, when known, belongs to its service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationIntentRecord {
    attempt: ReconciliationAttemptRecord,
    target: ReconciliationTarget,
    before: Option<ReconciliationIncarnation>,
    startup_failure: Option<StartupFailureRecoveryAuthority>,
}

impl ReconciliationIntentRecord {
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    pub fn new(
        attempt: ReconciliationAttemptRecord,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
    ) -> Result<Self, ReconciliationConsistencyError> {
        Self::with_startup_failure(attempt, target, before, None)
    }

    pub(crate) fn with_startup_failure(
        attempt: ReconciliationAttemptRecord,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
        startup_failure: Option<StartupFailureRecoveryAuthority>,
    ) -> Result<Self, ReconciliationConsistencyError> {
        if before
            .as_ref()
            .is_some_and(|before| before.target().service() != target.service())
        {
            return Err(ReconciliationConsistencyError::PriorServiceMismatch);
        }
        if startup_failure.as_ref().is_some_and(|authority| {
            authority.target().service() != target.service() || before.is_some()
        }) {
            return Err(ReconciliationConsistencyError::InvalidStartupFailureAuthority);
        }
        Ok(Self {
            attempt,
            target,
            before,
            startup_failure,
        })
    }
    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }
    pub fn before(&self) -> Option<&ReconciliationIncarnation> {
        self.before.as_ref()
    }
    pub fn startup_failure(&self) -> Option<&StartupFailureRecoveryAuthority> {
        self.startup_failure.as_ref()
    }
}

/// One ordered native decision or confirmed external fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationHistoryFact {
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    RetirementAcknowledged,
    OldServiceUnloaded,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    ServiceDefinitionPublished,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    ServiceDefinitionDurable,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    BootstrapCommandRequested,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    BootstrapCommandCompleted,
    BootstrapCommandSucceeded,
}

/// A validated ordered prefix of one native reconciliation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconciliationHistory(Vec<ReconciliationHistoryFact>);

impl ReconciliationHistory {
    fn assess(
        before: Option<&ReconciliationIncarnation>,
        facts: &[ReconciliationHistoryFact],
    ) -> ReconciliationHistoryAssessment {
        let mut trusted = Vec::new();
        for &fact in facts {
            if !accepts_next(before.is_some(), &trusted, fact) {
                return ReconciliationHistoryAssessment::Rejected {
                    trusted: Self(trusted),
                    rejected: fact,
                };
            }
            trusted.push(fact);
        }
        ReconciliationHistoryAssessment::Accepted(Self(trusted))
    }

    pub fn facts(&self) -> &[ReconciliationHistoryFact] {
        &self.0
    }

    pub fn proves_unloaded(&self) -> bool {
        self.0
            .contains(&ReconciliationHistoryFact::OldServiceUnloaded)
    }
}

/// Result of validating an adapter's ordered history report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationHistoryAssessment {
    Accepted(ReconciliationHistory),
    Rejected {
        trusted: ReconciliationHistory,
        rejected: ReconciliationHistoryFact,
    },
}

/// Terminal physical report supplied by the native adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationPhysicalRecord {
    Confirmed(ReconciliationIncarnation),
    Failed,
}

/// Whether durable intent delivery completed before native effects were allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationIntentDeliveryRecord {
    Reserved,
    Acknowledged,
    Failed,
}

/// Earliest native effect timing relative to immutable intent acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationEffectTimingRecord {
    NoEffectsObserved,
    BeforeIntentReservation,
    BeforeIntentAcknowledgement,
    AfterIntentAcknowledgement,
}

/// Relationship between the claimed physical incarnation and the admitted intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationRuntimeIdentity {
    NotReported,
    HealthyReuse,
    FreshIncarnation,
    Replacement,
    ReusedRuntimeInstance,
    ChangedProcessForRuntimeInstance,
}

/// Independent consistency facts derived from one terminal adapter report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationValidationFacts {
    rejected_history_fact: Option<ReconciliationHistoryFact>,
    history_complete: bool,
    target_matches: bool,
    runtime_identity: ReconciliationRuntimeIdentity,
    physical_report_agrees: bool,
    effect_timing_matches_history: bool,
    effects_followed_intent: bool,
    candidate_eligible: bool,
}

impl ReconciliationValidationFacts {
    #[cfg(test)]
    pub fn rejected_history_fact(&self) -> Option<ReconciliationHistoryFact> {
        self.rejected_history_fact
    }

    #[cfg(test)]
    pub fn target_matches(&self) -> bool {
        self.target_matches
    }

    #[cfg(test)]
    pub fn runtime_identity(&self) -> ReconciliationRuntimeIdentity {
        self.runtime_identity
    }

    #[cfg(test)]
    pub fn effect_timing_matches_history(&self) -> bool {
        self.effect_timing_matches_history
    }

    #[cfg(test)]
    pub fn effects_followed_intent(&self) -> bool {
        self.effects_followed_intent
    }

    #[cfg(test)]
    pub fn candidate_eligible(&self) -> bool {
        self.candidate_eligible
    }
}

/// How the host may update its prior physical authority after assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationCleanupDecision {
    RetainPrior,
    ClearPrior,
    AdoptClaimed,
}

/// A rejected terminal report with its original claim and independent checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationRejectedReport {
    claimed_physical: ReconciliationPhysicalRecord,
    validation: ReconciliationValidationFacts,
    cleanup: ReconciliationCleanupDecision,
}

impl ReconciliationRejectedReport {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "rejected-report inspection is exercised by domain and application tests; production serializes the enclosing outcome"
        )
    )]
    pub fn claimed_physical(&self) -> &ReconciliationPhysicalRecord {
        &self.claimed_physical
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "rejected-report inspection is exercised by domain and application tests; production serializes the enclosing outcome"
        )
    )]
    pub fn claimed_identity(&self) -> Option<&ReconciliationIncarnation> {
        match &self.claimed_physical {
            ReconciliationPhysicalRecord::Confirmed(identity) => Some(identity),
            ReconciliationPhysicalRecord::Failed => None,
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "rejected-report inspection is exercised by domain and application tests; production serializes the enclosing outcome"
        )
    )]
    pub fn validation(&self) -> &ReconciliationValidationFacts {
        &self.validation
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "rejected-report inspection is exercised by domain and application tests; production serializes the enclosing outcome"
        )
    )]
    pub fn cleanup(&self) -> ReconciliationCleanupDecision {
        self.cleanup
    }
}

/// Whether the terminal report agrees with the admitted intent and native history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationOutcomeDisposition {
    Accepted(ReconciliationPhysicalRecord),
    Rejected(ReconciliationRejectedReport),
}

/// Validated agreement between an admitted intent and its physical result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationOutcomeRecord {
    intent: ReconciliationIntentRecord,
    intent_delivery: ReconciliationIntentDeliveryRecord,
    effect_timing: ReconciliationEffectTimingRecord,
    physical: ReconciliationPhysicalRecord,
    reported_history: Vec<ReconciliationHistoryFact>,
    history: ReconciliationHistory,
    validation: ReconciliationValidationFacts,
    cleanup: ReconciliationCleanupDecision,
    disposition: ReconciliationOutcomeDisposition,
}

impl ReconciliationOutcomeRecord {
    pub fn assess(
        intent: ReconciliationIntentRecord,
        intent_delivery: ReconciliationIntentDeliveryRecord,
        effect_timing: ReconciliationEffectTimingRecord,
        facts: Vec<ReconciliationHistoryFact>,
        physical: ReconciliationPhysicalRecord,
    ) -> Self {
        let reported_history = facts;
        let (history, rejected_history_fact) =
            match ReconciliationHistory::assess(intent.before(), &reported_history) {
                ReconciliationHistoryAssessment::Accepted(history) => (history, None),
                ReconciliationHistoryAssessment::Rejected { trusted, rejected } => {
                    (trusted, Some(rejected))
                }
            };
        let history_complete = history.facts().last()
            == Some(&ReconciliationHistoryFact::BootstrapCommandSucceeded)
            || (history.facts().is_empty() && intent.before().is_some());
        let target_matches = match &physical {
            ReconciliationPhysicalRecord::Confirmed(after) => after.target() == intent.target(),
            ReconciliationPhysicalRecord::Failed => true,
        };
        let runtime_identity = runtime_identity(intent.before(), &history, &physical);
        let physical_report_agrees = match &physical {
            ReconciliationPhysicalRecord::Confirmed(_) => history_complete,
            ReconciliationPhysicalRecord::Failed => rejected_history_fact.is_none(),
        };
        let expected_runtime_identity = if history.facts().is_empty() {
            ReconciliationRuntimeIdentity::HealthyReuse
        } else if intent.before().is_some() {
            ReconciliationRuntimeIdentity::Replacement
        } else {
            ReconciliationRuntimeIdentity::FreshIncarnation
        };
        let history_proves_candidate = rejected_history_fact.is_none()
            || history.facts().last()
                == Some(&ReconciliationHistoryFact::BootstrapCommandSucceeded);
        let candidate_eligible = matches!(physical, ReconciliationPhysicalRecord::Confirmed(_))
            && history_proves_candidate
            && target_matches
            && physical_report_agrees
            && runtime_identity == expected_runtime_identity;
        let effect_timing_matches_history = matches!(
            (reported_history.is_empty(), effect_timing),
            (true, ReconciliationEffectTimingRecord::NoEffectsObserved)
                | (
                    false,
                    ReconciliationEffectTimingRecord::BeforeIntentReservation
                        | ReconciliationEffectTimingRecord::BeforeIntentAcknowledgement
                        | ReconciliationEffectTimingRecord::AfterIntentAcknowledgement
                )
        );
        let effects_followed_intent = matches!(
            effect_timing,
            ReconciliationEffectTimingRecord::NoEffectsObserved
                | ReconciliationEffectTimingRecord::AfterIntentAcknowledgement
        );
        let validation = ReconciliationValidationFacts {
            rejected_history_fact,
            history_complete,
            target_matches,
            runtime_identity,
            physical_report_agrees,
            effect_timing_matches_history,
            effects_followed_intent,
            candidate_eligible,
        };
        let cleanup = if candidate_eligible {
            ReconciliationCleanupDecision::AdoptClaimed
        } else if history.proves_unloaded() {
            ReconciliationCleanupDecision::ClearPrior
        } else {
            ReconciliationCleanupDecision::RetainPrior
        };
        let disposition = if (candidate_eligible
            && intent_delivery == ReconciliationIntentDeliveryRecord::Acknowledged
            && effect_timing_matches_history
            && effects_followed_intent
            && rejected_history_fact.is_none())
            || (matches!(physical, ReconciliationPhysicalRecord::Failed)
                && rejected_history_fact.is_none()
                && effect_timing_matches_history
                && physical_report_agrees)
        {
            ReconciliationOutcomeDisposition::Accepted(physical.clone())
        } else {
            ReconciliationOutcomeDisposition::Rejected(ReconciliationRejectedReport {
                claimed_physical: physical.clone(),
                validation: validation.clone(),
                cleanup,
            })
        };
        Self {
            intent,
            intent_delivery,
            effect_timing,
            physical,
            reported_history,
            history,
            validation,
            cleanup,
            disposition,
        }
    }

    #[cfg(test)]
    pub fn reported_history(&self) -> &[ReconciliationHistoryFact] {
        &self.reported_history
    }

    pub fn history(&self) -> &ReconciliationHistory {
        &self.history
    }

    #[cfg(test)]
    pub fn validation(&self) -> &ReconciliationValidationFacts {
        &self.validation
    }

    pub fn cleanup(&self) -> ReconciliationCleanupDecision {
        self.cleanup
    }

    pub fn disposition(&self) -> &ReconciliationOutcomeDisposition {
        &self.disposition
    }
}

/// Pending causal authority that only its owning attempt may discharge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReconciliation {
    attempt: ReconciliationAttemptRecord,
}

impl PendingReconciliation {
    pub fn new(attempt: ReconciliationAttemptRecord) -> Self {
        Self { attempt }
    }
    pub fn is_owned_by(&self, attempt: &ReconciliationAttemptRecord) -> bool {
        attempt == &self.attempt
    }
}

/// Related reconciliation evidence contradicted the lifecycle contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationConsistencyError {
    EqualRequestAndAttempt,
    #[cfg_attr(
        all(not(any(target_os = "macos", target_os = "linux")), not(test)),
        allow(
            dead_code,
            reason = "native reconciliation is supported only on macOS and Linux"
        )
    )]
    PriorServiceMismatch,
    InvalidStartupFailureAuthority,
}

impl Display for ReconciliationConsistencyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EqualRequestAndAttempt => {
                "gateway request and attempt correlations must be distinct"
            }
            Self::PriorServiceMismatch => {
                "gateway reconciliation intent disagrees with its prior service"
            }
            Self::InvalidStartupFailureAuthority => {
                "gateway startup-failure authority contradicts its reconciliation intent"
            }
        })
    }
}

impl Error for ReconciliationConsistencyError {}

fn accepts_next(
    had_managed_incarnation: bool,
    trusted: &[ReconciliationHistoryFact],
    next: ReconciliationHistoryFact,
) -> bool {
    let previous = trusted.last().copied();
    matches!(
        (previous, next),
        (None, ReconciliationHistoryFact::RetirementAcknowledged) if had_managed_incarnation
    ) || matches!(
        (previous, next),
        (None, ReconciliationHistoryFact::OldServiceUnloaded) if !had_managed_incarnation
    ) || matches!(
        (previous, next),
        (None, ReconciliationHistoryFact::ServiceDefinitionPublished) if !had_managed_incarnation
    ) || matches!(
        (previous, next),
        (
            Some(ReconciliationHistoryFact::RetirementAcknowledged),
            ReconciliationHistoryFact::OldServiceUnloaded
        ) | (
            Some(ReconciliationHistoryFact::OldServiceUnloaded),
            ReconciliationHistoryFact::ServiceDefinitionPublished
        ) | (
            Some(ReconciliationHistoryFact::ServiceDefinitionPublished),
            ReconciliationHistoryFact::ServiceDefinitionDurable
        ) | (
            Some(ReconciliationHistoryFact::ServiceDefinitionDurable),
            ReconciliationHistoryFact::BootstrapCommandRequested
        ) | (
            Some(ReconciliationHistoryFact::BootstrapCommandRequested),
            ReconciliationHistoryFact::BootstrapCommandCompleted
        ) | (
            Some(ReconciliationHistoryFact::BootstrapCommandCompleted),
            ReconciliationHistoryFact::BootstrapCommandSucceeded
        )
    )
}

fn runtime_identity(
    before: Option<&ReconciliationIncarnation>,
    history: &ReconciliationHistory,
    physical: &ReconciliationPhysicalRecord,
) -> ReconciliationRuntimeIdentity {
    let ReconciliationPhysicalRecord::Confirmed(after) = physical else {
        return ReconciliationRuntimeIdentity::NotReported;
    };
    let Some(before) = before else {
        return ReconciliationRuntimeIdentity::FreshIncarnation;
    };
    if before.runtime_instance() != after.runtime_instance() {
        return ReconciliationRuntimeIdentity::Replacement;
    }
    if before.process_id() != after.process_id() {
        return ReconciliationRuntimeIdentity::ChangedProcessForRuntimeInstance;
    }
    if history.facts().is_empty() && before == after {
        ReconciliationRuntimeIdentity::HealthyReuse
    } else {
        ReconciliationRuntimeIdentity::ReusedRuntimeInstance
    }
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

fn sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_actions_require_a_verified_bundled_surface() {
        for cause in [
            ReconciliationCause::CredentialLoad,
            ReconciliationCause::ExplicitRetry,
        ] {
            assert_eq!(
                ReconciliationEvidence::new(cause, ReconciliationInitiator::DesktopHost),
                Err(ReconciliationEvidenceError)
            );
            assert!(ReconciliationEvidence::new(
                cause,
                ReconciliationInitiator::BundledSurface(BundledSurface::Main)
            )
            .is_ok());
        }
        assert!(ReconciliationEvidence::new(
            ReconciliationCause::ClaudeConfigurationChanged,
            ReconciliationInitiator::DesktopHost
        )
        .is_ok());
        assert!(ReconciliationEvidence::new(
            ReconciliationCause::ClaudeConfigurationChanged,
            ReconciliationInitiator::BundledSurface(BundledSurface::Main)
        )
        .is_ok());
        assert!(ReconciliationEvidence::new(
            ReconciliationCause::Startup,
            ReconciliationInitiator::DesktopHost
        )
        .is_ok());
    }

    #[test]
    fn correlations_are_canonical_lowercase_uuids() {
        let first = ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
            .expect("first");
        let second =
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440001".into())
                .expect("second");
        assert!(first.require_distinct_from(&second).is_ok());
        assert_eq!(
            first.require_distinct_from(&first),
            Err(ReconciliationCorrelationPairError)
        );
        assert!(
            ReconciliationCorrelation::parse("550E8400-E29B-41D4-A716-446655440000".into())
                .is_err()
        );
    }

    #[test]
    fn target_and_incarnation_reject_ambiguous_identity() {
        let target = ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .expect("target");
        assert!(ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440000".into(),
            42,
            7420
        )
        .is_ok());
        assert_eq!(
            ReconciliationTarget::new("service".into(), "short".into(), "b".repeat(64)),
            Err(ReconciliationIdentityError)
        );
        assert_eq!(
            ReconciliationIncarnation::new(target, String::new(), 0, 0),
            Err(ReconciliationIdentityError)
        );
    }

    fn correlation(serial: u64) -> ReconciliationCorrelation {
        ReconciliationCorrelation::parse(format!("00000000-0000-4000-8000-{serial:012x}"))
            .expect("correlation")
    }

    fn target(service: &str, fingerprint: char) -> ReconciliationTarget {
        ReconciliationTarget::new(
            service.into(),
            fingerprint.to_string().repeat(64),
            "b".repeat(64),
        )
        .expect("target")
    }

    fn incarnation(
        target: ReconciliationTarget,
        serial: u64,
        process_id: u32,
    ) -> ReconciliationIncarnation {
        ReconciliationIncarnation::new(
            target,
            format!("00000000-0000-4000-8000-{serial:012x}"),
            process_id,
            7420,
        )
        .expect("incarnation")
    }

    fn intent(
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
    ) -> ReconciliationIntentRecord {
        let request = ReconciliationRequestRecord::new(
            correlation(1),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .expect("evidence"),
        );
        let attempt = ReconciliationAttemptRecord::new(correlation(2), request).expect("attempt");
        ReconciliationIntentRecord::new(attempt, target, before).expect("intent")
    }

    fn replacement_history() -> Vec<ReconciliationHistoryFact> {
        vec![
            ReconciliationHistoryFact::RetirementAcknowledged,
            ReconciliationHistoryFact::OldServiceUnloaded,
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::BootstrapCommandSucceeded,
        ]
    }

    fn installation_history(unload_first: bool) -> Vec<ReconciliationHistoryFact> {
        let mut facts = if unload_first {
            vec![ReconciliationHistoryFact::OldServiceUnloaded]
        } else {
            Vec::new()
        };
        facts.extend([
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::BootstrapCommandSucceeded,
        ]);
        facts
    }

    fn assess(
        intent: ReconciliationIntentRecord,
        history: Vec<ReconciliationHistoryFact>,
        physical: ReconciliationPhysicalRecord,
    ) -> ReconciliationOutcomeRecord {
        let effect_timing = if history.is_empty() {
            ReconciliationEffectTimingRecord::NoEffectsObserved
        } else {
            ReconciliationEffectTimingRecord::AfterIntentAcknowledgement
        };
        ReconciliationOutcomeRecord::assess(
            intent,
            ReconciliationIntentDeliveryRecord::Acknowledged,
            effect_timing,
            history,
            physical,
        )
    }

    #[test]
    fn request_attempt_and_pending_authority_remain_distinct() {
        let request = ReconciliationRequestRecord::new(
            correlation(1),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        assert_eq!(
            ReconciliationAttemptRecord::new(correlation(1), request.clone()),
            Err(ReconciliationConsistencyError::EqualRequestAndAttempt)
        );
        let attempt = ReconciliationAttemptRecord::new(correlation(2), request).unwrap();
        let pending = PendingReconciliation::new(attempt.clone());
        assert!(pending.is_owned_by(&attempt));
        let other_request = ReconciliationRequestRecord::new(
            correlation(3),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let other = ReconciliationAttemptRecord::new(correlation(4), other_request).unwrap();
        assert!(!pending.is_owned_by(&other));
    }

    #[test]
    fn accepts_healthy_reuse_fresh_install_replacement_and_unknown_unload() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let reuse = assess(
            intent(expected.clone(), Some(before.clone())),
            Vec::new(),
            ReconciliationPhysicalRecord::Confirmed(before),
        );
        assert!(matches!(
            reuse.disposition(),
            ReconciliationOutcomeDisposition::Accepted(_)
        ));
        assert_eq!(reuse.cleanup(), ReconciliationCleanupDecision::AdoptClaimed);

        for unload_first in [false, true] {
            let fresh = assess(
                intent(expected.clone(), None),
                installation_history(unload_first),
                ReconciliationPhysicalRecord::Confirmed(incarnation(expected.clone(), 11, 43)),
            );
            assert!(matches!(
                fresh.disposition(),
                ReconciliationOutcomeDisposition::Accepted(_)
            ));
        }

        let before = incarnation(expected.clone(), 12, 44);
        let replacement = assess(
            intent(expected.clone(), Some(before)),
            replacement_history(),
            ReconciliationPhysicalRecord::Confirmed(incarnation(expected, 13, 45)),
        );
        assert!(matches!(
            replacement.disposition(),
            ReconciliationOutcomeDisposition::Accepted(_)
        ));
    }

    #[test]
    fn every_valid_failed_prefix_remains_a_failure_report() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let replacement = replacement_history();
        for length in 0..=replacement.len() {
            let outcome = assess(
                intent(expected.clone(), Some(before.clone())),
                replacement[..length].to_vec(),
                ReconciliationPhysicalRecord::Failed,
            );
            assert!(matches!(
                outcome.disposition(),
                ReconciliationOutcomeDisposition::Accepted(ReconciliationPhysicalRecord::Failed)
            ));
        }
        for unload_first in [false, true] {
            let installation = installation_history(unload_first);
            for length in 0..=installation.len() {
                let outcome = assess(
                    intent(expected.clone(), None),
                    installation[..length].to_vec(),
                    ReconciliationPhysicalRecord::Failed,
                );
                assert!(matches!(
                    outcome.disposition(),
                    ReconciliationOutcomeDisposition::Accepted(
                        ReconciliationPhysicalRecord::Failed
                    )
                ));
            }
        }
    }

    #[test]
    fn known_incarnation_cannot_be_replaced_without_retirement_and_unload() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let outcome = assess(
            intent(expected.clone(), Some(before)),
            installation_history(false),
            ReconciliationPhysicalRecord::Confirmed(incarnation(expected, 11, 43)),
        );
        let ReconciliationOutcomeDisposition::Rejected(report) = outcome.disposition() else {
            panic!("replacement without retirement was accepted");
        };
        assert_eq!(
            report.validation().rejected_history_fact(),
            Some(ReconciliationHistoryFact::ServiceDefinitionPublished)
        );
        assert!(!report.validation().candidate_eligible());
        assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
    }

    #[test]
    fn malformed_history_does_not_mask_identity_contradictions_or_claim() {
        let expected = target("service", 'a');
        let other = target("other", 'c');
        let before = incarnation(expected.clone(), 10, 42);
        let changed_process = incarnation(other, 10, 99);
        let reported = vec![
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::ServiceDefinitionPublished,
        ];
        let outcome = assess(
            intent(expected, Some(before)),
            reported.clone(),
            ReconciliationPhysicalRecord::Confirmed(changed_process.clone()),
        );
        assert_eq!(outcome.reported_history(), reported);
        assert!(outcome.history().facts().is_empty());
        let ReconciliationOutcomeDisposition::Rejected(report) = outcome.disposition() else {
            panic!("contradictory report was accepted");
        };
        assert_eq!(report.claimed_identity(), Some(&changed_process));
        assert!(!report.validation().target_matches());
        assert_eq!(
            report.validation().runtime_identity(),
            ReconciliationRuntimeIdentity::ChangedProcessForRuntimeInstance
        );
        assert_eq!(
            report.validation().rejected_history_fact(),
            Some(ReconciliationHistoryFact::BootstrapCommandCompleted)
        );
        assert_eq!(report.cleanup(), ReconciliationCleanupDecision::RetainPrior);
    }

    #[test]
    fn reused_runtime_instance_is_independent_of_malformed_history() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let outcome = assess(
            intent(expected, Some(before.clone())),
            vec![
                ReconciliationHistoryFact::RetirementAcknowledged,
                ReconciliationHistoryFact::BootstrapCommandSucceeded,
            ],
            ReconciliationPhysicalRecord::Confirmed(before),
        );
        assert_eq!(
            outcome.validation().runtime_identity(),
            ReconciliationRuntimeIdentity::ReusedRuntimeInstance
        );
        assert_eq!(
            outcome.validation().rejected_history_fact(),
            Some(ReconciliationHistoryFact::BootstrapCommandSucceeded)
        );
        assert!(!outcome.validation().candidate_eligible());
    }

    #[test]
    fn invalid_history_cannot_turn_exact_identity_into_healthy_reuse() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let outcome = assess(
            intent(expected, Some(before.clone())),
            vec![ReconciliationHistoryFact::BootstrapCommandSucceeded],
            ReconciliationPhysicalRecord::Confirmed(before),
        );
        assert_eq!(
            outcome.validation().runtime_identity(),
            ReconciliationRuntimeIdentity::HealthyReuse
        );
        assert!(!outcome.validation().candidate_eligible());
        assert_eq!(
            outcome.cleanup(),
            ReconciliationCleanupDecision::RetainPrior
        );
    }

    #[test]
    fn failed_intent_delivery_blocks_acceptance_but_preserves_cleanup_authority() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let after = incarnation(expected.clone(), 11, 43);
        let outcome = ReconciliationOutcomeRecord::assess(
            intent(expected, Some(before)),
            ReconciliationIntentDeliveryRecord::Failed,
            ReconciliationEffectTimingRecord::BeforeIntentAcknowledgement,
            replacement_history(),
            ReconciliationPhysicalRecord::Confirmed(after.clone()),
        );
        let ReconciliationOutcomeDisposition::Rejected(report) = outcome.disposition() else {
            panic!("failed intent delivery accepted physical success");
        };
        assert_eq!(
            report.claimed_physical(),
            &ReconciliationPhysicalRecord::Confirmed(after.clone())
        );
        assert_eq!(report.claimed_identity(), Some(&after));
        assert!(report.validation().candidate_eligible());
        assert_eq!(
            report.cleanup(),
            ReconciliationCleanupDecision::AdoptClaimed
        );
    }

    #[test]
    fn effect_timing_must_agree_with_reported_history() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let outcome = ReconciliationOutcomeRecord::assess(
            intent(expected, Some(before)),
            ReconciliationIntentDeliveryRecord::Acknowledged,
            ReconciliationEffectTimingRecord::NoEffectsObserved,
            vec![ReconciliationHistoryFact::RetirementAcknowledged],
            ReconciliationPhysicalRecord::Failed,
        );
        assert!(!outcome.validation().effect_timing_matches_history());
        assert!(matches!(
            outcome.disposition(),
            ReconciliationOutcomeDisposition::Rejected(_)
        ));
    }

    #[test]
    fn invalid_suffix_is_retained_without_erasing_proven_candidate() {
        let expected = target("service", 'a');
        let before = incarnation(expected.clone(), 10, 42);
        let after = incarnation(expected.clone(), 11, 43);
        let mut reported = replacement_history();
        reported.push(ReconciliationHistoryFact::BootstrapCommandSucceeded);
        let outcome = assess(
            intent(expected, Some(before)),
            reported.clone(),
            ReconciliationPhysicalRecord::Confirmed(after.clone()),
        );
        assert_eq!(outcome.reported_history(), reported);
        assert_eq!(outcome.history().facts(), replacement_history());
        let ReconciliationOutcomeDisposition::Rejected(report) = outcome.disposition() else {
            panic!("invalid suffix was accepted");
        };
        assert_eq!(report.claimed_identity(), Some(&after));
        assert!(report.validation().candidate_eligible());
        assert_eq!(
            report.cleanup(),
            ReconciliationCleanupDecision::AdoptClaimed
        );
    }
}
