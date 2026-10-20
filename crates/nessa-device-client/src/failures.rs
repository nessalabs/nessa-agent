//! Immutable foreign failure views preserve compound publication evidence.
use crate::{native_product::NativeProductFailure, session::SessionFailure};
use nessa_auth::{
    adapters::pairing::PairingCryptoError,
    application::pairing::{
        GatewayPublicationState, PairingWorkerFault, PrivatePublicationEffect,
        PrivatePublicationError, PrivatePublicationStep, PrivateStateError, PrivateStorageFailure,
    },
    domain::pairing::PairingError,
};
use nessa_client_core::{pairing::NativeClientError, retained::ReadFailure};
use nessa_protocol::pairing::wire::NativeWireError;
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FormatResult},
    sync::Arc,
};

macro_rules! copied_enum {
    ($name:ident, $source:path, [$($variant:ident),+ $(,)?]) => {
        #[doc = "Existing core enum values, translated by an exhaustive From implementation."]
        #[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
        pub enum $name { $(#[doc = "Existing same-named core variant."] $variant),+ }
        impl From<$source> for $name {
            fn from(value: $source) -> Self {
                use $source as Source;
                match value { $(Source::$variant => Self::$variant),+ }
            }
        }
    };
}
copied_enum!(
    NativeCryptoFailure,
    PairingCryptoError,
    [
        InvalidCode,
        InvalidContext,
        InvalidProof,
        Unavailable,
        PendingStorage
    ]
);
copied_enum!(
    NativePairingFailure,
    PairingError,
    [
        Invalid,
        Expired,
        AttemptsExhausted,
        Capacity,
        AvailableSlotOccupied,
        Conflict,
        Ineligible,
        WrongActor,
        StaleGeneration
    ]
);
copied_enum!(NativeWorkerFault, PairingWorkerFault, [Panic, Cancelled]);
copied_enum!(
    NativeReadFailure,
    ReadFailure,
    [
        Refused,
        Unreachable,
        Away,
        ResetRequired,
        Protocol,
        Cache,
        CacheUncertain,
        CacheDamaged,
        Quota,
        Stopped,
        CursorAhead,
        SubscriptionCapacity
    ]
);
copied_enum!(
    NativeKeyPublication,
    GatewayPublicationState,
    [NotPublished, Published]
);
copied_enum!(
    NativeStorageFailure,
    PrivateStorageFailure,
    [UnsafeStorage, Unavailable]
);
copied_enum!(
    NativePublicationStep,
    PrivatePublicationStep,
    [
        ValidateDestination,
        VerifyOriginBinding,
        ValidateReservation,
        FlushBeforeRename,
        Rename,
        FlushAfterRename,
        ValidatePublishedDestination,
        VerifyPublishedBinding,
        SyncDirectory
    ]
);
copied_enum!(
    NativePublicationEffect,
    PrivatePublicationEffect,
    [NotPublished, Published]
);

/// One immutable original publication failure, including independent cleanup.
#[derive(Debug, uniffi::Object)]
pub struct NativePublicationFailure {
    value: PrivatePublicationError,
}
#[uniffi::export]
impl NativePublicationFailure {
    /// Original failed checkpoint.
    pub fn step(&self) -> NativePublicationStep {
        self.value.step().into()
    }
    /// Original storage failure.
    pub fn primary(&self) -> NativeStorageFailure {
        self.value.primary().into()
    }
    /// Independent reservation cleanup failure.
    pub fn cleanup(&self) -> Option<NativeStorageFailure> {
        self.value.cleanup().map(Into::into)
    }
    /// Physical rename fact, independently of acknowledgement.
    pub fn effect(&self) -> NativePublicationEffect {
        self.value.effect().into()
    }
    /// Independent later acknowledgement failure.
    pub fn reconciliation(&self) -> Option<NativeStorageFailure> {
        self.value.reconciliation().map(Into::into)
    }
}
/// Private-state failure variant; payloads belong to the same immutable object.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativePrivateStateKind {
    /// Key effect exists independently of unavailable audit evidence.
    AuditUnavailable,
    /// Native key publication failure with physical/cleanup evidence.
    Publication,
    /// Native audit publication failure with independent key effect.
    AuditPublication,
    /// Unsafe or malformed private state.
    Corrupt,
    /// Exact binding or intent differs.
    Conflict,
    /// Another private-state owner holds the lock.
    Locked,
    /// Failure before publication was known.
    Unavailable,
    /// Publication occurred but acknowledgement remains uncertain.
    Uncertain,
}
/// Immutable private-state failure; expected effects are not flattened away.
#[derive(Debug, uniffi::Object)]
pub struct NativePrivateStateFailure {
    value: PrivateStateError,
}
impl NativePrivateStateFailure {
    pub(crate) fn from_state(value: PrivateStateError) -> Self {
        Self { value }
    }
}
#[uniffi::export]
impl NativePrivateStateFailure {
    /// Original variant.
    pub fn kind(&self) -> NativePrivateStateKind {
        match self.value {
            PrivateStateError::AuditUnavailable(_) => NativePrivateStateKind::AuditUnavailable,
            PrivateStateError::Publication(_) => NativePrivateStateKind::Publication,
            PrivateStateError::AuditPublication { .. } => NativePrivateStateKind::AuditPublication,
            PrivateStateError::Corrupt => NativePrivateStateKind::Corrupt,
            PrivateStateError::Conflict => NativePrivateStateKind::Conflict,
            PrivateStateError::Locked => NativePrivateStateKind::Locked,
            PrivateStateError::Unavailable => NativePrivateStateKind::Unavailable,
            PrivateStateError::Uncertain => NativePrivateStateKind::Uncertain,
        }
    }
    /// Original key publication fact for audit-related failures.
    pub fn key_publication(&self) -> Option<NativeKeyPublication> {
        match self.value {
            PrivateStateError::AuditUnavailable(key)
            | PrivateStateError::AuditPublication { key, .. } => Some(key.into()),
            _ => None,
        }
    }
    /// Original native publication failure, when this variant contains one.
    pub fn publication(&self) -> Option<Arc<NativePublicationFailure>> {
        match self.value {
            PrivateStateError::Publication(value)
            | PrivateStateError::AuditPublication { failure: value, .. } => {
                Some(Arc::new(NativePublicationFailure { value }))
            }
            _ => None,
        }
    }
}
/// Existing wire failure variants, with correlation cause retained separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeWireFailureKind {
    /// Encoded envelope bound.
    TooLarge,
    /// Invalid current representation.
    Invalid,
    /// Exact operation/disclosure contradiction.
    Correlation,
}
/// Immutable wire failure containing the original domain correlation payload.
#[derive(Debug, uniffi::Object)]
pub struct NativeWireFailure {
    value: NativeWireError,
}
#[uniffi::export]
impl NativeWireFailure {
    /// Original wire variant.
    pub fn kind(&self) -> NativeWireFailureKind {
        match self.value {
            NativeWireError::TooLarge => NativeWireFailureKind::TooLarge,
            NativeWireError::Invalid => NativeWireFailureKind::Invalid,
            NativeWireError::Correlation(_) => NativeWireFailureKind::Correlation,
        }
    }
    /// Original typed domain failure for correlation variants.
    pub fn correlation(&self) -> Option<NativePairingFailure> {
        match self.value {
            NativeWireError::Correlation(value) => Some(value.into()),
            _ => None,
        }
    }
}
/// Existing enrollment failure variant; getters retain its original payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeEnrollmentFailureKind {
    /// Physical capacity occupied or shutdown signalled.
    Busy,
    /// Exact pending enrollment already exists.
    PendingExists,
    /// Issued credential already exists.
    Enrolled,
    /// Original canonical receipt does not permit retry.
    OriginalNotRetryable,
    /// No pending or issued record.
    NoPending,
    /// Private-state failure.
    Storage,
    /// Failure after TLS handshake.
    Crypto,
    /// TLS handshake failure, distinct from wrong-code proof.
    Handshake,
    /// Native bounded wire refusal.
    Wire,
    /// Peer phase contradicts expected operation.
    Phase,
    /// Invitation enrolls another class of party.
    OtherEnrollee,
    /// Redacted gateway refusal.
    Refused,
    /// Socket failure; diagnostic OS kind grants no cleanup authority.
    Io,
    /// Fallible entropy acquisition refused.
    Entropy,
    /// Unexpected physical worker termination.
    WorkerFault,
}
/// Immutable existing enrollment failure. Diagnostic text is not a policy input.
#[derive(Debug, uniffi::Object)]
pub struct NativeEnrollmentFailure {
    pub(crate) value: NativeClientError,
}
#[uniffi::export]
impl NativeEnrollmentFailure {
    /// Existing variant; strings do not select failure policy.
    pub fn kind(&self) -> NativeEnrollmentFailureKind {
        match self.value {
            NativeClientError::Busy => NativeEnrollmentFailureKind::Busy,
            NativeClientError::PendingExists => NativeEnrollmentFailureKind::PendingExists,
            NativeClientError::Enrolled => NativeEnrollmentFailureKind::Enrolled,
            NativeClientError::OriginalNotRetryable => {
                NativeEnrollmentFailureKind::OriginalNotRetryable
            }
            NativeClientError::NoPending => NativeEnrollmentFailureKind::NoPending,
            NativeClientError::Storage(_) => NativeEnrollmentFailureKind::Storage,
            NativeClientError::Crypto(_) => NativeEnrollmentFailureKind::Crypto,
            NativeClientError::Handshake(_) => NativeEnrollmentFailureKind::Handshake,
            NativeClientError::Wire(_) => NativeEnrollmentFailureKind::Wire,
            NativeClientError::Phase => NativeEnrollmentFailureKind::Phase,
            NativeClientError::OtherEnrollee => NativeEnrollmentFailureKind::OtherEnrollee,
            NativeClientError::Refused => NativeEnrollmentFailureKind::Refused,
            NativeClientError::Io(_) => NativeEnrollmentFailureKind::Io,
            NativeClientError::Entropy => NativeEnrollmentFailureKind::Entropy,
            NativeClientError::WorkerFault(_) => NativeEnrollmentFailureKind::WorkerFault,
        }
    }
    /// Original private-state failure and all of its effects.
    pub fn storage(&self) -> Option<Arc<NativePrivateStateFailure>> {
        match self.value {
            NativeClientError::Storage(value) => {
                Some(Arc::new(NativePrivateStateFailure { value }))
            }
            _ => None,
        }
    }
    /// Original crypto failure for either crypto or handshake variant.
    pub fn crypto(&self) -> Option<NativeCryptoFailure> {
        match self.value {
            NativeClientError::Crypto(value) | NativeClientError::Handshake(value) => {
                Some(value.into())
            }
            _ => None,
        }
    }
    /// Original bounded wire failure and correlation payload.
    pub fn wire(&self) -> Option<Arc<NativeWireFailure>> {
        match self.value {
            NativeClientError::Wire(value) => Some(Arc::new(NativeWireFailure { value })),
            _ => None,
        }
    }
    /// Original unexpected worker fault.
    pub fn worker(&self) -> Option<NativeWorkerFault> {
        match self.value {
            NativeClientError::WorkerFault(value) => Some(value.into()),
            _ => None,
        }
    }
    /// OS ErrorKind for diagnosis only; retry/cleanup policy uses typed variants.
    pub fn io_diagnostic(&self) -> Option<String> {
        match self.value {
            NativeClientError::Io(value) => Some(format!("{value:?}")),
            _ => None,
        }
    }
}
/// Session failure family. Its immutable payload getters preserve original effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeSessionFailureKind {
    /// Concurrent admission refused.
    Busy,
    /// Scope explicitly closed.
    Closed,
    /// Synchronization owner invalidated by an unexpected fault.
    Damaged,
    /// Runtime creation failed.
    Runtime,
    /// Private-state operation failed.
    PrivateState,
    /// Saved-cache operation failed.
    Cache,
    /// Enrollment failed without a compound completed status.
    Enrollment,
    /// No issued receiver to select.
    NoCredential,
    /// Stop preceded status dispatch.
    Stopped,
    /// Protected product opening or physical drain failed.
    Product,
    /// Original recovery or later actual owner selection refused.
    Owner,
    /// The publication stamp does not name the bound receiver.
    Fenced,
}
/// Why a phone publication stamp cannot read.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativePhoneFence {
    /// No receiver is bound.
    Unbound,
    /// The session is closed.
    Closed,
    /// A newer receiver replaced this stamp.
    Replaced,
}
impl From<nessa_client_core::retained::PhoneFence> for NativePhoneFence {
    fn from(value: nessa_client_core::retained::PhoneFence) -> Self {
        match value {
            nessa_client_core::retained::PhoneFence::Unbound => Self::Unbound,
            nessa_client_core::retained::PhoneFence::Closed => Self::Closed,
            nessa_client_core::retained::PhoneFence::Replaced => Self::Replaced,
        }
    }
}
/// Immutable original session failure, safe to retain after session deallocation.
#[derive(Debug, uniffi::Object)]
pub struct NativeSessionFailure {
    pub(crate) value: SessionFailure,
}
#[uniffi::export]
impl NativeSessionFailure {
    /// Original family.
    pub fn kind(&self) -> NativeSessionFailureKind {
        match self.value {
            SessionFailure::Busy => NativeSessionFailureKind::Busy,
            SessionFailure::Closed => NativeSessionFailureKind::Closed,
            SessionFailure::Damaged => NativeSessionFailureKind::Damaged,
            SessionFailure::Runtime(_) => NativeSessionFailureKind::Runtime,
            SessionFailure::PrivateState(_) => NativeSessionFailureKind::PrivateState,
            SessionFailure::Cache(_) => NativeSessionFailureKind::Cache,
            SessionFailure::Enrollment(_) => NativeSessionFailureKind::Enrollment,
            SessionFailure::NoCredential => NativeSessionFailureKind::NoCredential,
            SessionFailure::Stopped => NativeSessionFailureKind::Stopped,
            SessionFailure::Product(_) => NativeSessionFailureKind::Product,
            SessionFailure::Owner(_) | SessionFailure::OwnerSelection { .. } => {
                NativeSessionFailureKind::Owner
            }
            SessionFailure::Fenced(_) => NativeSessionFailureKind::Fenced,
        }
    }
    /// Why a publication stamp was refused. Empty for every other failure.
    pub fn fence(&self) -> Option<NativePhoneFence> {
        match self.value {
            SessionFailure::Fenced(value) => Some(value.into()),
            _ => None,
        }
    }
    /// Original private-state failure.
    pub fn private_state(&self) -> Option<Arc<NativePrivateStateFailure>> {
        match self.value {
            SessionFailure::PrivateState(value) => {
                Some(Arc::new(NativePrivateStateFailure { value }))
            }
            _ => None,
        }
    }
    /// Original saved-cache failure.
    pub fn cache(&self) -> Option<NativeReadFailure> {
        match self.value {
            SessionFailure::Cache(value) => Some(value.into()),
            _ => None,
        }
    }
    /// Original enrollment failure.
    pub fn enrollment(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        match self.value {
            SessionFailure::Enrollment(value) => Some(Arc::new(NativeEnrollmentFailure { value })),
            _ => None,
        }
    }
    /// Original product failure, including the original optional typed close reason.
    pub fn product(&self) -> Option<Arc<NativeProductFailure>> {
        match self.value {
            SessionFailure::Product(value) => Some(Arc::new(NativeProductFailure { value })),
            _ => None,
        }
    }
    /// Entire owner recovery/settlement facts preserved when product selection refused.
    pub fn owner_status(&self) -> Option<Arc<crate::native_session::NativeStatus>> {
        match &self.value {
            SessionFailure::Owner(value) | SessionFailure::OwnerSelection { status: value, .. } => {
                Some(Arc::new(crate::native_session::NativeStatus {
                    value: (**value).clone(),
                }))
            }
            _ => None,
        }
    }
    /// Actual later selected-record refusal, separate from successful earlier status.
    pub fn owner_selection(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        match self.value {
            SessionFailure::OwnerSelection { error, .. } => {
                Some(Arc::new(NativeEnrollmentFailure { value: error }))
            }
            _ => None,
        }
    }
    /// OS ErrorKind for runtime diagnosis, not a policy selector.
    pub fn runtime_diagnostic(&self) -> Option<String> {
        match self.value {
            SessionFailure::Runtime(value) => Some(format!("{value:?}")),
            _ => None,
        }
    }
}
/// Foreign call failure. Compound session effects remain immutable objects.
#[derive(Debug, uniffi::Error)]
pub enum NativeCallError {
    /// Foreign argument representation cannot form the existing typed input.
    InvalidArgument,
    /// Original session failure, including independent effect payloads.
    Session {
        /// Original owned failure.
        failure: Arc<NativeSessionFailure>,
    },
}
impl From<SessionFailure> for NativeCallError {
    fn from(value: SessionFailure) -> Self {
        Self::Session {
            failure: Arc::new(NativeSessionFailure { value }),
        }
    }
}
impl Display for NativeCallError {
    fn fmt(&self, output: &mut Formatter<'_>) -> FormatResult {
        write!(output, "{self:?}")
    }
}
impl Error for NativeCallError {}

#[cfg(test)]
#[path = "../tests/session/failures.rs"]
mod tests;
