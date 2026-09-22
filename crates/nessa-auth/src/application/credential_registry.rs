//! Credential-registry refusal evidence and its durable audit boundary.
//!
//! Opening the registry is an infrastructure effect. When its stored
//! representation is unsafe to trust, the application records that refusal
//! without changing the file. The caller supplies why the read happened and
//! who is known to have initiated it; the storage adapter supplies record
//! identity and observation time.

use std::{
    fmt,
    path::{Path, PathBuf},
};

/// A safe diagnostic for registry bytes that this build refuses to trust.
///
/// Variants carry structural locations and limits only. They never retain a
/// rejected credential value or a serde diagnostic that could echo one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialRegistryFault {
    /// JSON could not be decoded. Line and column locate the syntax without
    /// copying any rejected value into logs or audit storage.
    MalformedJson {
        /// One-based line reported by the JSON decoder.
        line: usize,
        /// One-based column reported by the JSON decoder.
        column: usize,
        /// Safe decoder category.
        category: JsonFaultCategory,
    },
    /// The file belongs to a registry schema this build does not read.
    UnsupportedSchema {
        /// Schema found in the file.
        found: u64,
        /// Only schema accepted by this build.
        expected: u32,
    },
    /// The representation decoded but violated a named registry invariant.
    InvalidState(RegistryInvariant),
    /// The file exceeded the configured read bound.
    TooLarge {
        /// Observed size when it was available without reading past the bound.
        observed_bytes: u64,
        /// Configured maximum registry size.
        maximum_bytes: u64,
    },
    /// An authoritative registry file failed the private-storage boundary.
    UnsafeStorage(CredentialRegistryStorageRole),
}

/// The authority-bearing file that failed private-storage validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialRegistryStorageRole {
    /// The credential state and lifecycle evidence file.
    Registry,
    /// The sibling lifetime lock that establishes one registry owner.
    Lock,
}

impl CredentialRegistryStorageRole {
    /// Return the stable audit representation of the file's authority role.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Lock => "lock",
        }
    }
}

/// Safe categories from `serde_json`; no rejected value is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonFaultCategory {
    Io,
    Syntax,
    Data,
    Eof,
}

impl JsonFaultCategory {
    /// Return the stable, secret-free audit representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::Syntax => "syntax",
            Self::Data => "data_shape",
            Self::Eof => "unexpected_end",
        }
    }
}

/// The invariant family rejected by the registry's one validator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryInvariant {
    Capacity,
    GatewayIdentity,
    OwnerMembership,
    DuplicateIdentity,
    MembershipBinding,
    CredentialMetadata,
    CredentialBinding,
    CredentialVerifier,
    CommandReceipt,
    TransitionHistory,
}

impl RegistryInvariant {
    /// Return the stable audit name of the rejected invariant family.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::GatewayIdentity => "gateway_identity",
            Self::OwnerMembership => "owner_membership",
            Self::DuplicateIdentity => "duplicate_identity",
            Self::MembershipBinding => "membership_binding",
            Self::CredentialMetadata => "credential_metadata",
            Self::CredentialBinding => "credential_binding",
            Self::CredentialVerifier => "credential_verifier",
            Self::CommandReceipt => "command_receipt",
            Self::TransitionHistory => "transition_history",
        }
    }
}

impl fmt::Display for CredentialRegistryFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedJson {
                line,
                column,
                category,
            } => write!(
                formatter,
                "JSON {} at line {line}, column {column}",
                category.as_str()
            ),
            Self::UnsupportedSchema { found, expected } => {
                write!(
                    formatter,
                    "schema {found} is unsupported; expected schema {expected}"
                )
            }
            Self::InvalidState(rule) => {
                write!(formatter, "the {} invariant was violated", rule.as_str())
            }
            Self::TooLarge {
                observed_bytes,
                maximum_bytes,
            } => write!(
                formatter,
                "file size {observed_bytes} bytes exceeds the {maximum_bytes}-byte limit"
            ),
            Self::UnsafeStorage(role) => write!(
                formatter,
                "{} file or path is not private, single-linked storage owned by the current OS user",
                role.as_str()
            ),
        }
    }
}

/// Why the process attempted to open the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialRegistryRefusalCause {
    GatewayStartup,
    AutomaticProvisioning,
    LocalAuthCommand,
}

impl CredentialRegistryRefusalCause {
    /// Return the stable audit representation of the opening lifecycle.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GatewayStartup => "gateway_startup",
            Self::AutomaticProvisioning => "automatic_local_provisioning",
            Self::LocalAuthCommand => "local_auth_command",
        }
    }
}

/// What is honestly known about who initiated the registry read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialRegistryRefusalInitiator {
    /// The gateway or desktop provisioning lifecycle performed the read.
    Automatic,
    /// A local command process performed the read. No human identity is
    /// invented because this boundary has no verified OS principal.
    LocalProcess,
}

impl CredentialRegistryRefusalInitiator {
    /// Return the stable audit representation of the known initiator.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::LocalProcess => "local_process_unattributed",
        }
    }
}

/// Immutable evidence that a registry was refused and left untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRegistryRefusal {
    target: PathBuf,
    fault: CredentialRegistryFault,
    cause: CredentialRegistryRefusalCause,
    initiator: CredentialRegistryRefusalInitiator,
}

impl CredentialRegistryRefusal {
    /// Build refusal evidence from the exact target, safe fault, cause, and
    /// honestly known initiator supplied at the application boundary.
    pub fn new(
        target: PathBuf,
        fault: CredentialRegistryFault,
        cause: CredentialRegistryRefusalCause,
        initiator: CredentialRegistryRefusalInitiator,
    ) -> Self {
        Self {
            target,
            fault,
            cause,
            initiator,
        }
    }

    /// Return the registry path that was refused and preserved.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Return the safe structural fault reported by the storage adapter.
    pub fn fault(&self) -> &CredentialRegistryFault {
        &self.fault
    }

    /// Return why this process attempted to open the registry.
    pub fn cause(&self) -> CredentialRegistryRefusalCause {
        self.cause
    }

    /// Return the verified level of initiator attribution.
    pub fn initiator(&self) -> CredentialRegistryRefusalInitiator {
        self.initiator
    }
}

/// Failure to commit refusal evidence. It contains no registry bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRegistryAuditError(String);

impl CredentialRegistryAuditError {
    /// Report that the sink could not durably commit refusal evidence.
    ///
    /// The detail must describe only the audit sink failure and must not copy
    /// registry contents.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl fmt::Display for CredentialRegistryAuditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CredentialRegistryAuditError {}

/// Durable sink for a refusal before the caller reports startup failure.
pub trait CredentialRegistryRefusalAudit: Send + Sync {
    /// Durably record one refusal before returning success.
    ///
    /// An error means audit durability was not established. The caller keeps
    /// the original registry fault as the primary failure in either case.
    fn record(
        &self,
        refusal: &CredentialRegistryRefusal,
    ) -> Result<(), CredentialRegistryAuditError>;
}
