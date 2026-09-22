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
                ReconciliationCause::CredentialLoad
                    | ReconciliationCause::ExplicitRetry
                    | ReconciliationCause::ClaudeConfigurationChanged,
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

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }

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
    pub fn correlation(&self) -> &ReconciliationCorrelation {
        &self.correlation
    }
    pub fn origin(&self) -> &ReconciliationRequestRecord {
        &self.origin
    }
}

/// Validated intent whose prior incarnation, when known, belongs to its service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationIntentRecord {
    attempt: ReconciliationAttemptRecord,
    target: ReconciliationTarget,
    before: Option<ReconciliationIncarnation>,
}

impl ReconciliationIntentRecord {
    pub fn new(
        attempt: ReconciliationAttemptRecord,
        target: ReconciliationTarget,
        before: Option<ReconciliationIncarnation>,
    ) -> Result<Self, ReconciliationConsistencyError> {
        if before
            .as_ref()
            .is_some_and(|before| before.target().service() != target.service())
        {
            return Err(ReconciliationConsistencyError::PriorServiceMismatch);
        }
        Ok(Self {
            attempt,
            target,
            before,
        })
    }
    pub fn attempt(&self) -> &ReconciliationAttemptRecord {
        &self.attempt
    }
    pub fn target(&self) -> &ReconciliationTarget {
        &self.target
    }
    pub fn before(&self) -> Option<&ReconciliationIncarnation> {
        self.before.as_ref()
    }
    pub fn validate_confirmed(
        &self,
        after: &ReconciliationIncarnation,
    ) -> Result<(), ReconciliationConsistencyError> {
        if after.target() == &self.target {
            Ok(())
        } else {
            Err(ReconciliationConsistencyError::ConfirmedTargetMismatch)
        }
    }
}

/// One ordered native decision or confirmed external fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationHistoryFact {
    RetirementAcknowledged,
    OldServiceUnloaded,
    ServiceDefinitionPublished,
    ServiceDefinitionDurable,
    BootstrapCommandRequested,
    BootstrapCommandCompleted,
    BootstrapCommandSucceeded,
}

/// A validated ordered prefix of one native reconciliation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconciliationHistory(Vec<ReconciliationHistoryFact>);

impl ReconciliationHistory {
    pub fn assess(facts: Vec<ReconciliationHistoryFact>) -> ReconciliationHistoryAssessment {
        let mut trusted = Vec::new();
        for fact in facts {
            if !accepts_next(trusted.last().copied(), fact) {
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

    pub fn permits_confirmation(&self) -> bool {
        self.0.is_empty()
            || self.0.last() == Some(&ReconciliationHistoryFact::BootstrapCommandSucceeded)
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

/// Why a terminal adapter report was rejected by domain consistency rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationRejectedReport {
    InvalidHistory(ReconciliationHistoryFact),
    ConfirmationWithoutCompleteHistory,
    ConfirmedTargetMismatch(ReconciliationIncarnation),
    HealthyReuseMismatch(ReconciliationIncarnation),
    ReusedRuntimeInstance(ReconciliationIncarnation),
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
    history: ReconciliationHistory,
    disposition: ReconciliationOutcomeDisposition,
}

impl ReconciliationOutcomeRecord {
    pub fn assess(
        intent: ReconciliationIntentRecord,
        facts: Vec<ReconciliationHistoryFact>,
        physical: ReconciliationPhysicalRecord,
    ) -> Self {
        let confirmed_target_mismatch = match &physical {
            ReconciliationPhysicalRecord::Confirmed(after) if after.target() != intent.target() => {
                Some(after.clone())
            }
            _ => None,
        };
        let (history, disposition) = match ReconciliationHistory::assess(facts) {
            ReconciliationHistoryAssessment::Rejected { trusted, rejected } => (
                trusted,
                ReconciliationOutcomeDisposition::Rejected(match confirmed_target_mismatch {
                    Some(after) => ReconciliationRejectedReport::ConfirmedTargetMismatch(after),
                    None => ReconciliationRejectedReport::InvalidHistory(rejected),
                }),
            ),
            ReconciliationHistoryAssessment::Accepted(history) => {
                let disposition = match confirmed_target_mismatch {
                    Some(after) => ReconciliationOutcomeDisposition::Rejected(
                        ReconciliationRejectedReport::ConfirmedTargetMismatch(after),
                    ),
                    None if matches!(&physical, ReconciliationPhysicalRecord::Confirmed(_))
                        && !history.permits_confirmation() =>
                    {
                        ReconciliationOutcomeDisposition::Rejected(
                            ReconciliationRejectedReport::ConfirmationWithoutCompleteHistory,
                        )
                    }
                    None if matches!(&physical, ReconciliationPhysicalRecord::Confirmed(after)
                            if history.facts().is_empty()
                                && intent.before() != Some(after)) =>
                    {
                        let ReconciliationPhysicalRecord::Confirmed(after) = &physical else {
                            unreachable!()
                        };
                        ReconciliationOutcomeDisposition::Rejected(
                            ReconciliationRejectedReport::HealthyReuseMismatch(after.clone()),
                        )
                    }
                    None if matches!(&physical, ReconciliationPhysicalRecord::Confirmed(after)
                            if !history.facts().is_empty()
                                && intent.before().is_some_and(|before|
                                    before.runtime_instance() == after.runtime_instance())) =>
                    {
                        let ReconciliationPhysicalRecord::Confirmed(after) = &physical else {
                            unreachable!()
                        };
                        ReconciliationOutcomeDisposition::Rejected(
                            ReconciliationRejectedReport::ReusedRuntimeInstance(after.clone()),
                        )
                    }
                    None => ReconciliationOutcomeDisposition::Accepted(physical),
                };
                (history, disposition)
            }
        };
        Self {
            intent,
            history,
            disposition,
        }
    }
    pub fn intent(&self) -> &ReconciliationIntentRecord {
        &self.intent
    }
    pub fn history(&self) -> &ReconciliationHistory {
        &self.history
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
    pub fn attempt(&self) -> &ReconciliationAttemptRecord {
        &self.attempt
    }
    pub fn is_owned_by(&self, attempt: &ReconciliationAttemptRecord) -> bool {
        attempt == &self.attempt
    }
}

/// Related reconciliation evidence contradicted the lifecycle contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationConsistencyError {
    EqualRequestAndAttempt,
    PriorServiceMismatch,
    ConfirmedTargetMismatch,
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
            Self::ConfirmedTargetMismatch => {
                "gateway reconciliation outcome disagrees with admitted target"
            }
        })
    }
}

impl Error for ReconciliationConsistencyError {}

fn accepts_next(
    previous: Option<ReconciliationHistoryFact>,
    next: ReconciliationHistoryFact,
) -> bool {
    matches!(
        (previous, next),
        (None, ReconciliationHistoryFact::RetirementAcknowledged)
            | (None, ReconciliationHistoryFact::OldServiceUnloaded)
            | (None, ReconciliationHistoryFact::ServiceDefinitionPublished)
            | (
                Some(ReconciliationHistoryFact::RetirementAcknowledged),
                ReconciliationHistoryFact::OldServiceUnloaded
            )
            | (
                Some(ReconciliationHistoryFact::OldServiceUnloaded),
                ReconciliationHistoryFact::ServiceDefinitionPublished
            )
            | (
                Some(ReconciliationHistoryFact::ServiceDefinitionPublished),
                ReconciliationHistoryFact::ServiceDefinitionDurable
            )
            | (
                Some(ReconciliationHistoryFact::ServiceDefinitionDurable),
                ReconciliationHistoryFact::BootstrapCommandRequested
            )
            | (
                Some(ReconciliationHistoryFact::BootstrapCommandRequested),
                ReconciliationHistoryFact::BootstrapCommandCompleted
            )
            | (
                Some(ReconciliationHistoryFact::BootstrapCommandCompleted),
                ReconciliationHistoryFact::BootstrapCommandSucceeded
            )
    )
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
            ReconciliationCause::ClaudeConfigurationChanged,
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

    #[test]
    fn related_evidence_and_pending_authority_agree_as_one_domain_rule() {
        let request = ReconciliationRequestRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
                .unwrap(),
            ReconciliationEvidence::new(
                ReconciliationCause::ClaudeConfigurationChanged,
                ReconciliationInitiator::BundledSurface(BundledSurface::Main),
            )
            .unwrap(),
        );
        assert_eq!(
            ReconciliationAttemptRecord::new(request.correlation().clone(), request.clone()),
            Err(ReconciliationConsistencyError::EqualRequestAndAttempt)
        );
        let attempt = ReconciliationAttemptRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440001".into())
                .unwrap(),
            request.clone(),
        )
        .unwrap();
        let pending = PendingReconciliation::new(attempt.clone());
        assert!(pending.is_owned_by(&attempt));
        let predecessor_request = ReconciliationRequestRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440004".into())
                .unwrap(),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let predecessor = ReconciliationAttemptRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440005".into())
                .unwrap(),
            predecessor_request,
        )
        .unwrap();
        assert!(!pending.is_owned_by(&predecessor));

        let target =
            ReconciliationTarget::new("service".into(), "a".repeat(64), "b".repeat(64)).unwrap();
        let other =
            ReconciliationTarget::new("other".into(), "a".repeat(64), "b".repeat(64)).unwrap();
        let before = ReconciliationIncarnation::new(
            other.clone(),
            "550e8400-e29b-41d4-a716-446655440002".into(),
            7,
            7420,
        )
        .unwrap();
        assert_eq!(
            ReconciliationIntentRecord::new(attempt.clone(), target.clone(), Some(before)),
            Err(ReconciliationConsistencyError::PriorServiceMismatch)
        );
        let intent = ReconciliationIntentRecord::new(attempt, target, None).unwrap();
        let after = ReconciliationIncarnation::new(
            other,
            "550e8400-e29b-41d4-a716-446655440003".into(),
            8,
            7420,
        )
        .unwrap();
        assert_eq!(
            intent.validate_confirmed(&after),
            Err(ReconciliationConsistencyError::ConfirmedTargetMismatch)
        );
        let rejected = ReconciliationOutcomeRecord::assess(
            intent.clone(),
            vec![
                ReconciliationHistoryFact::BootstrapCommandCompleted,
                ReconciliationHistoryFact::ServiceDefinitionPublished,
            ],
            ReconciliationPhysicalRecord::Failed,
        );
        assert!(matches!(
            rejected.disposition(),
            ReconciliationOutcomeDisposition::Rejected(
                ReconciliationRejectedReport::InvalidHistory(
                    ReconciliationHistoryFact::BootstrapCommandCompleted
                )
            )
        ));
        assert!(rejected.history().facts().is_empty());

        let incomplete_success = ReconciliationOutcomeRecord::assess(
            intent,
            vec![
                ReconciliationHistoryFact::ServiceDefinitionPublished,
                ReconciliationHistoryFact::ServiceDefinitionDurable,
            ],
            ReconciliationPhysicalRecord::Confirmed(after),
        );
        assert!(matches!(
            incomplete_success.disposition(),
            ReconciliationOutcomeDisposition::Rejected(
                ReconciliationRejectedReport::ConfirmedTargetMismatch(_)
            )
        ));
    }

    #[test]
    fn confirmation_preserves_incarnation_meaning_across_reuse_and_replacement() {
        let request = ReconciliationRequestRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440010".into())
                .unwrap(),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let attempt = ReconciliationAttemptRecord::new(
            ReconciliationCorrelation::parse("550e8400-e29b-41d4-a716-446655440011".into())
                .unwrap(),
            request,
        )
        .unwrap();
        let target =
            ReconciliationTarget::new("service".into(), "a".repeat(64), "b".repeat(64)).unwrap();
        let before = ReconciliationIncarnation::new(
            target.clone(),
            "550e8400-e29b-41d4-a716-446655440012".into(),
            42,
            7420,
        )
        .unwrap();
        let intent =
            ReconciliationIntentRecord::new(attempt, target.clone(), Some(before.clone())).unwrap();
        assert!(matches!(
            ReconciliationOutcomeRecord::assess(
                intent.clone(),
                Vec::new(),
                ReconciliationPhysicalRecord::Confirmed(before.clone()),
            )
            .disposition(),
            ReconciliationOutcomeDisposition::Accepted(_)
        ));
        let replacement_history = vec![
            ReconciliationHistoryFact::RetirementAcknowledged,
            ReconciliationHistoryFact::OldServiceUnloaded,
            ReconciliationHistoryFact::ServiceDefinitionPublished,
            ReconciliationHistoryFact::ServiceDefinitionDurable,
            ReconciliationHistoryFact::BootstrapCommandRequested,
            ReconciliationHistoryFact::BootstrapCommandCompleted,
            ReconciliationHistoryFact::BootstrapCommandSucceeded,
        ];
        let reused = ReconciliationOutcomeRecord::assess(
            intent,
            replacement_history,
            ReconciliationPhysicalRecord::Confirmed(before),
        );
        assert!(matches!(
            reused.disposition(),
            ReconciliationOutcomeDisposition::Rejected(
                ReconciliationRejectedReport::ReusedRuntimeInstance(_)
            )
        ));
    }
}
