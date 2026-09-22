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
}
