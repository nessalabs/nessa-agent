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

/// Confirmed native boundaries observed during one reconciliation attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationNativeEffect {
    RetirementAcknowledged,
    OldServiceUnloaded,
    ServiceDefinitionPublished,
    ServiceDefinitionDurable,
    BootstrapCommandCompleted,
    BootstrapCommandSucceeded,
}

/// Local native decisions retained without presenting them as external effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationNativeDecision {
    BootstrapCommandRequested,
}

/// Physical evidence retained independently from diagnostic error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationPhysicalRecord {
    Confirmed(ReconciliationIncarnation),
    Refused,
    Partial(Vec<ReconciliationNativeEffect>),
}

/// Validated agreement between an admitted intent and its physical result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationOutcomeRecord {
    intent: ReconciliationIntentRecord,
    decisions: Vec<ReconciliationNativeDecision>,
    physical: ReconciliationPhysicalRecord,
}

impl ReconciliationOutcomeRecord {
    pub fn new(
        intent: ReconciliationIntentRecord,
        decisions: Vec<ReconciliationNativeDecision>,
        physical: ReconciliationPhysicalRecord,
    ) -> Result<Self, ReconciliationConsistencyError> {
        if !valid_decision_history(&decisions) {
            return Err(ReconciliationConsistencyError::InvalidEffectHistory);
        }
        match &physical {
            ReconciliationPhysicalRecord::Confirmed(after) => {
                if !decisions.is_empty() {
                    return Err(ReconciliationConsistencyError::InvalidEffectHistory);
                }
                intent.validate_confirmed(after)?;
            }
            ReconciliationPhysicalRecord::Partial(effects)
                if !valid_effect_history(&decisions, effects) =>
            {
                return Err(ReconciliationConsistencyError::InvalidEffectHistory);
            }
            _ => {}
        }
        Ok(Self {
            intent,
            decisions,
            physical,
        })
    }
    pub fn intent(&self) -> &ReconciliationIntentRecord {
        &self.intent
    }
    pub fn physical(&self) -> &ReconciliationPhysicalRecord {
        &self.physical
    }
    pub fn decisions(&self) -> &[ReconciliationNativeDecision] {
        &self.decisions
    }
}

/// Pending causal authority that only its owning attempt may discharge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingReconciliation {
    request: ReconciliationRequestRecord,
}

impl PendingReconciliation {
    pub fn new(request: ReconciliationRequestRecord) -> Self {
        Self { request }
    }
    pub fn request(&self) -> &ReconciliationRequestRecord {
        &self.request
    }
    pub fn is_owned_by(&self, attempt: &ReconciliationAttemptRecord) -> bool {
        attempt.origin() == &self.request
    }
}

/// Related reconciliation evidence contradicted the lifecycle contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationConsistencyError {
    EqualRequestAndAttempt,
    PriorServiceMismatch,
    ConfirmedTargetMismatch,
    InvalidEffectHistory,
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
            Self::InvalidEffectHistory => {
                "partial gateway reconciliation has contradictory native effects"
            }
        })
    }
}

impl Error for ReconciliationConsistencyError {}

fn valid_decision_history(decisions: &[ReconciliationNativeDecision]) -> bool {
    decisions.is_empty() || decisions == [ReconciliationNativeDecision::BootstrapCommandRequested]
}

fn valid_effect_history(
    decisions: &[ReconciliationNativeDecision],
    effects: &[ReconciliationNativeEffect],
) -> bool {
    if effects.is_empty() {
        return false;
    }
    let mut index = 0;
    let retired = effects.first() == Some(&ReconciliationNativeEffect::RetirementAcknowledged);
    if retired {
        index += 1;
    }
    let unloaded = effects.get(index) == Some(&ReconciliationNativeEffect::OldServiceUnloaded);
    if unloaded {
        index += 1;
    }
    if index == effects.len() {
        return decisions.is_empty();
    }
    if retired && !unloaded {
        return false;
    }
    let installation = [
        ReconciliationNativeEffect::ServiceDefinitionPublished,
        ReconciliationNativeEffect::ServiceDefinitionDurable,
        ReconciliationNativeEffect::BootstrapCommandCompleted,
        ReconciliationNativeEffect::BootstrapCommandSucceeded,
    ];
    let observed_installation = &effects[index..];
    if observed_installation.len() > installation.len()
        || observed_installation != &installation[..observed_installation.len()]
    {
        return false;
    }
    let requested = decisions.contains(&ReconciliationNativeDecision::BootstrapCommandRequested);
    let durable = effects.contains(&ReconciliationNativeEffect::ServiceDefinitionDurable);
    let completed = effects.contains(&ReconciliationNativeEffect::BootstrapCommandCompleted);
    (!requested || durable) && (!completed || requested)
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
        let pending = PendingReconciliation::new(request.clone());
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
        assert_eq!(
            ReconciliationOutcomeRecord::new(
                intent.clone(),
                Vec::new(),
                ReconciliationPhysicalRecord::Partial(Vec::new()),
            ),
            Err(ReconciliationConsistencyError::InvalidEffectHistory)
        );
        assert_eq!(
            ReconciliationOutcomeRecord::new(
                ReconciliationIntentRecord::new(
                    ReconciliationAttemptRecord::new(
                        ReconciliationCorrelation::parse(
                            "550e8400-e29b-41d4-a716-446655440006".into()
                        )
                        .unwrap(),
                        request,
                    )
                    .unwrap(),
                    ReconciliationTarget::new("service".into(), "a".repeat(64), "b".repeat(64))
                        .unwrap(),
                    None,
                )
                .unwrap(),
                vec![ReconciliationNativeDecision::BootstrapCommandRequested],
                ReconciliationPhysicalRecord::Partial(vec![
                    ReconciliationNativeEffect::BootstrapCommandCompleted,
                    ReconciliationNativeEffect::ServiceDefinitionPublished,
                ]),
            ),
            Err(ReconciliationConsistencyError::InvalidEffectHistory)
        );
        assert_eq!(
            ReconciliationOutcomeRecord::new(
                intent,
                Vec::new(),
                ReconciliationPhysicalRecord::Partial(vec![
                    ReconciliationNativeEffect::ServiceDefinitionPublished,
                    ReconciliationNativeEffect::ServiceDefinitionDurable,
                    ReconciliationNativeEffect::BootstrapCommandCompleted,
                ]),
            ),
            Err(ReconciliationConsistencyError::InvalidEffectHistory)
        );
    }
}
