//! Validated connection values and durable allocation facts. No IO or clocks.
use super::{AdmissionRejection, EnvironmentIdentity};
use nessa_vault::domain::Secret;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Locked,
    NotConfigured,
    InvalidInput,
    Storage,
    Provider,
    EnvironmentUnavailable,
    AdmissionRejected(AdmissionRejection),
    RefusedWorkspace {
        vm: String,
        rejection: AdmissionRejection,
        disposition: RefusalDisposition,
    },
    UnverifiedWorkspace {
        vm: String,
    },
    AdmissionRecovery {
        vm: String,
        rejection: AdmissionRejection,
        receipt_failed: bool,
        audit_failed: bool,
        cleanup: CleanupOutcome,
        cleanup_audit_failed: bool,
    },
    WorkerUnavailable,
    ProvisioningUncertain,
    Missing,
    CleanupUncertain,
    ReceiptRecovery {
        vm: String,
        receipt_audit_failed: bool,
        cleanup: CleanupOutcome,
        cleanup_audit_failed: bool,
    },
}

/// The user's infrastructure credential is transient, redacted and zeroizing.
#[derive(Clone)]
pub(crate) struct BoatConnection {
    pub(crate) token: Secret,
    pub(crate) environment: String,
    pub(crate) image: String,
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
        admission: EnvironmentIdentity,
    },
    /// No authority may ever be granted after a recorded admission refusal.
    Refused {
        thread: String,
        vm: String,
        image: String,
        admission: EnvironmentIdentity,
        rejection: AdmissionRejection,
        disposition: RefusalDisposition,
    },
    /// Reader-only old receipt; never upgraded or executed.
    Unverified {
        thread: String,
        vm: String,
        image: String,
        environment: String,
    },
}
impl Assignment {
    pub(crate) fn thread(&self) -> &str {
        match self {
            Self::Provisioning { thread }
            | Self::Assigned { thread, .. }
            | Self::Refused { thread, .. }
            | Self::Unverified { thread, .. } => thread,
        }
    }
}

impl Assignment {
    /// Receipt histories never change target or reopen a durably refused VM.
    pub(crate) fn validate_history(thread: &str, transitions: &[&Self]) -> Result<(), Failure> {
        let legal = match transitions {
            [Self::Provisioning { .. }] => true,
            [Self::Provisioning { .. }, Self::Assigned { .. } | Self::Refused { .. } | Self::Unverified { .. }] => {
                true
            }
            [Self::Provisioning { .. }, Self::Assigned {
                vm,
                image,
                admission,
                ..
            }, Self::Refused {
                vm: refused_vm,
                image: refused_image,
                admission: refused_admission,
                ..
            }] => vm == refused_vm && image == refused_image && admission == refused_admission,
            _ => false,
        };
        if !legal || transitions.iter().any(|a| a.thread() != thread) {
            return Err(Failure::Storage);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/workspaces/assignment_history.rs"]
mod tests;

/// Failed creation owns rollback; an existing workspace's data is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefusalDisposition {
    RollbackNewVm,
    RetainTarget,
}
impl RefusalDisposition {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RollbackNewVm => "rollback_new_vm",
            Self::RetainTarget => "retain_target",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CleanupOutcome {
    Retained,
    Unconfirmed,
    Confirmed,
}
