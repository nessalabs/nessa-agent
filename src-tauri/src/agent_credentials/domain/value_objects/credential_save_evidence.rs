use std::{error::Error, fmt};

use nessa_agent_credentials::{CredentialAgent, CredentialNamespace};

/// Bundled surface verified by the native command boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveCaller {
    Main,
    Setup,
}

/// Canonical request correlation for one credential-save lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSaveCorrelation(String);

impl CredentialSaveCorrelation {
    pub fn parse(value: String) -> Result<Self, CredentialSaveCorrelationError> {
        if canonical_uuid(&value) {
            Ok(Self(value))
        } else {
            Err(CredentialSaveCorrelationError)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Credential-save correlation was not a canonical lowercase UUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialSaveCorrelationError;

impl fmt::Display for CredentialSaveCorrelationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential save correlation must be a canonical UUID")
    }
}

impl Error for CredentialSaveCorrelationError {}

/// Immutable intent admitted before the secure-store write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSaveIntent {
    correlation: CredentialSaveCorrelation,
    caller: CredentialSaveCaller,
    target: CredentialSaveTarget,
}

impl CredentialSaveIntent {
    pub fn new(
        correlation: CredentialSaveCorrelation,
        caller: CredentialSaveCaller,
        target: CredentialSaveTarget,
    ) -> Self {
        Self {
            correlation,
            caller,
            target,
        }
    }

    pub fn correlation(&self) -> &CredentialSaveCorrelation {
        &self.correlation
    }

    pub fn caller(&self) -> CredentialSaveCaller {
        self.caller
    }

    pub fn agent(&self) -> CredentialAgent {
        self.target.agent()
    }

    pub fn namespace(&self) -> &CredentialNamespace {
        self.target.namespace()
    }

    pub fn target(&self) -> &CredentialSaveTarget {
        &self.target
    }
}

/// Exact canonical secure-store destination admitted with the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSaveTarget {
    agent: CredentialAgent,
    namespace: CredentialNamespace,
    service: String,
    account: String,
}

impl CredentialSaveTarget {
    pub fn new(
        agent: CredentialAgent,
        namespace: CredentialNamespace,
        service: String,
        account: String,
    ) -> Result<Self, CredentialSaveTargetError> {
        if invalid_external_name(&service) || invalid_external_name(&account) {
            return Err(CredentialSaveTargetError);
        }
        Ok(Self {
            agent,
            namespace,
            service,
            account,
        })
    }

    pub fn agent(&self) -> CredentialAgent {
        self.agent
    }
    pub fn namespace(&self) -> &CredentialNamespace {
        &self.namespace
    }
    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn account(&self) -> &str {
        &self.account
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialSaveTargetError;

impl fmt::Display for CredentialSaveTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential save target must use nonempty printable names")
    }
}

impl Error for CredentialSaveTargetError {}

/// Why the secure store refused a credential replacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveRefusal {
    Invalid,
}

/// Why a secure-store effect could not be classified as confirmed or refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveUncertainty {
    /// The store returned without confirming whether replacement occurred.
    StoreUnavailable,
    /// The store unwound after the replacement request was admitted.
    StorePanicked,
}

/// Secure-store effect, kept separate from audit delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveEffect {
    Confirmed,
    Refused(CredentialSaveRefusal),
    Uncertain(CredentialSaveUncertainty),
}

/// Final credential-save record with the exact admitted intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSaveOutcome {
    intent: CredentialSaveIntent,
    effect: CredentialSaveEffect,
}

impl CredentialSaveOutcome {
    pub fn new(intent: CredentialSaveIntent, effect: CredentialSaveEffect) -> Self {
        Self { intent, effect }
    }

    pub fn intent(&self) -> &CredentialSaveIntent {
        &self.intent
    }

    pub fn effect(&self) -> CredentialSaveEffect {
        self.effect
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

fn invalid_external_name(value: &str) -> bool {
    value.is_empty() || value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlations_are_canonical_and_evidence_contains_no_secret() {
        let correlation =
            CredentialSaveCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
                .unwrap();
        let namespace = CredentialNamespace::new("prod".into(), None).unwrap();
        let target = CredentialSaveTarget::new(
            CredentialAgent::Claude,
            namespace,
            "so.nessa.agent-credentials".into(),
            "prod:claude-api-key".into(),
        )
        .unwrap();
        let intent = CredentialSaveIntent::new(correlation, CredentialSaveCaller::Setup, target);

        assert_eq!(intent.agent(), CredentialAgent::Claude);
        assert_eq!(intent.namespace().stage(), "prod");
        assert_eq!(intent.target().account(), "prod:claude-api-key");
        assert!(
            CredentialSaveCorrelation::parse("550E8400-E29B-41D4-A716-446655440000".into())
                .is_err()
        );
    }
}
