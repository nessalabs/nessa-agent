//! Validated connection values and durable allocation facts. No IO or clocks.
use nessa_vault::domain::Secret;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Locked,
    NotConfigured,
    InvalidInput,
    Storage,
    Provider,
    EnvironmentUnavailable,
    WorkerUnavailable,
    ProvisioningUncertain,
    Missing,
    CleanupUncertain,
    ReceiptRecovery {
        vm: String,
        receipt_audit_failed: bool,
        cleanup_unconfirmed: bool,
        cleanup_audit_failed: bool,
    },
}

/// The user's infrastructure credential is transient, redacted and zeroizing.
#[derive(Clone)]
pub(crate) struct BoatConnection {
    pub(crate) token: Secret,
    pub(crate) environment: String,
    pub(crate) image: String,
    pub(crate) auto_stop: crate::conversation::domain::VmLifetime,
}
impl BoatConnection {
    pub(crate) fn new(token: String, environment: String, image: String) -> Result<Self, Failure> {
        let name = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if token.is_empty()
            || token.chars().count() > 4096
            || token.chars().any(char::is_whitespace)
            || !name(&environment)
            || !name(&image)
        {
            return Err(Failure::InvalidInput);
        }
        Ok(Self {
            token: Secret::new(token.into_bytes()),
            environment,
            image,
            auto_stop: Default::default(),
        })
    }
}

/// A saved fact, never a guess reconstructed from a missing provider resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Assignment {
    /// Intent is durable before create; a restart with only intent cannot create again.
    Provisioning { thread: String },
    Assigned {
        thread: String,
        vm: String,
        image: String,
        environment: String,
    },
}
impl Assignment {
    pub(crate) fn thread(&self) -> &str {
        match self {
            Self::Provisioning { thread } | Self::Assigned { thread, .. } => thread,
        }
    }
}
