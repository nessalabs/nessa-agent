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

/// Who Nessa is on Boat: the user's infrastructure token. Transient,
/// redacted and zeroizing. It says nothing about any VM.
#[derive(Clone)]
pub(crate) struct BoatAccount {
    pub(crate) token: Secret,
}

/// What one VM is made from and how long Boat lets it run unattended. A new
/// VM takes it from the saved connection ([`BoatConnection::new_vm`]); an
/// existing VM takes it from its receipt ([`VmSpec::of_receipt`]). The two
/// are never mixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VmSpec {
    pub(crate) image: String,
    pub(crate) environment: String,
    pub(crate) auto_stop: crate::conversation::domain::VmLifetime,
}
impl VmSpec {
    /// The spec of a VM already made: the image and environment it was
    /// admitted with, whatever the saved connection says now.
    pub(crate) fn of_receipt(
        image: String,
        admission: &EnvironmentIdentity,
        auto_stop: crate::conversation::domain::VmLifetime,
    ) -> Self {
        Self {
            image,
            environment: admission.name().into(),
            auto_stop,
        }
    }
}

/// The saved Boat connection: the account, and the environment and image
/// chosen when it was connected.
#[derive(Clone)]
pub(crate) struct BoatConnection {
    pub(crate) account: BoatAccount,
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
            account: BoatAccount {
                token: Secret::new(token.into_bytes()),
            },
            environment,
            image,
        })
    }
    /// The spec a new VM is made with: this connection's environment, and
    /// `pinned` (the image this build pins) or else the image saved here.
    pub(crate) fn new_vm(
        &self,
        pinned: Option<&str>,
        auto_stop: crate::conversation::domain::VmLifetime,
    ) -> VmSpec {
        VmSpec {
            image: pinned.unwrap_or(&self.image).to_owned(),
            environment: self.environment.clone(),
            auto_stop,
        }
    }
}

/// The owner's long-lived Claude sign-in for Boat VMs, as `claude setup-token`
/// mints it. Boat gives a VM made from an image none of the owner's own
/// logins, so this is what Claude in one runs with. It reaches a VM as one
/// line of a serve's standard input, so it holds only the token alphabet:
/// nothing that could end that line or be read as anything else.
#[derive(Clone)]
pub(crate) struct ClaudeSignIn(Secret);
impl ClaudeSignIn {
    const PREFIX: &'static str = "sk-ant-oat";
    const MAX_LEN: usize = 512;
    pub(crate) fn new(token: String) -> Result<Self, Failure> {
        if !token.starts_with(Self::PREFIX)
            || token.len() > Self::MAX_LEN
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Failure::InvalidInput);
        }
        Ok(Self(Secret::new(token.into_bytes())))
    }
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}
impl std::fmt::Debug for ClaudeSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClaudeSignIn(<redacted>)")
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
/// What a caller can do about a failed opening: the few cases a failure
/// comes down to. Each [`Failure`] is exactly one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenFailure {
    /// No Boat connection is saved, it was not valid, or its environment
    /// cannot run Nessa's agents: connect Boat again.
    NotConnected,
    /// The vault is locked: unlock it.
    Locked,
    /// Boat, or the gateway's own storage, failed: try again.
    ProviderFailed,
    /// The VM was refused by admission, or is gone, and never will be used:
    /// start the thread again.
    Refused,
    /// A VM may or may not exist or be cleaned up; nothing is created again
    /// on its own.
    Uncertain,
}
impl OpenFailure {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotConnected => "not_connected",
            Self::Locked => "locked",
            Self::ProviderFailed => "provider_failed",
            Self::Refused => "refused",
            Self::Uncertain => "uncertain",
        }
    }
}

impl Failure {
    /// Which of the few cases a caller acts on this failure is.
    pub(crate) fn cause(&self) -> OpenFailure {
        match self {
            Self::Locked => OpenFailure::Locked,
            Self::NotConfigured
            | Self::InvalidInput
            | Self::WorkerUnavailable
            | Self::EnvironmentUnavailable => OpenFailure::NotConnected,
            Self::Storage | Self::Provider => OpenFailure::ProviderFailed,
            // A thread with no VM to reopen, or one Boat no longer has, is
            // never fixed by trying again.
            Self::Missing
            | Self::AdmissionRejected(_)
            | Self::RefusedWorkspace { .. }
            | Self::UnverifiedWorkspace { .. } => OpenFailure::Refused,
            // A refusal is only settled once it is saved, recorded, and its
            // VM kept or confirmed gone.
            Self::AdmissionRecovery {
                receipt_failed: false,
                audit_failed: false,
                cleanup: CleanupOutcome::Retained | CleanupOutcome::Confirmed,
                cleanup_audit_failed: false,
                ..
            } => OpenFailure::Refused,
            Self::AdmissionRecovery { .. }
            | Self::ProvisioningUncertain
            | Self::CleanupUncertain
            | Self::ReceiptRecovery { .. } => OpenFailure::Uncertain,
        }
    }
    /// Its kind in a record: never what it carries.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::NotConfigured => "not_configured",
            Self::InvalidInput => "invalid_input",
            Self::Storage => "storage",
            Self::Provider => "provider",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::AdmissionRejected(_) => "admission_rejected",
            Self::RefusedWorkspace { .. } => "refused_workspace",
            Self::UnverifiedWorkspace { .. } => "unverified_workspace",
            Self::AdmissionRecovery { .. } => "admission_recovery",
            Self::WorkerUnavailable => "worker_unavailable",
            Self::ProvisioningUncertain => "provisioning_uncertain",
            Self::Missing => "missing",
            Self::CleanupUncertain => "cleanup_uncertain",
            Self::ReceiptRecovery { .. } => "receipt_recovery",
        }
    }
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
