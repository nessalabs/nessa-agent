//! Private key and pending recovery ports; these values confer no access authority.
use crate::{
    application::ports::Clock,
    domain::{pairing::PublicIntent, AudienceId},
};
use std::{error::Error, fmt};
use zeroize::Zeroizing;

/// One Nessa-owned seed; the owned copy is erased by Zeroizing on drop.
/// Deliberately non-cloneable, non-serializable and redacted in diagnostics.
pub struct PrivateKeyMaterial(Zeroizing<[u8; 32]>);
impl PrivateKeyMaterial {
    /// Take an owned seed from injected entropy or the private codec.
    pub fn new(seed: Zeroizing<[u8; 32]>) -> Self {
        Self(seed)
    }
    /// Borrow for signing or the private codec; callers must not log these bytes.
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Debug for PrivateKeyMaterial {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("PrivateKeyMaterial([REDACTED])")
    }
}

/// Atomic private recovery record, authenticated by valid KE2 before publication.
/// The local representation itself is not proof of a claim, approval or grant.
pub struct PendingEnrollment {
    key: PrivateKeyMaterial,
    gateway_pin: [u8; 44],
    intent: PublicIntent,
}
impl PendingEnrollment {
    /// Preserve key and public correlation together. The TLS owner validates SPKI.
    pub fn new(key: PrivateKeyMaterial, gateway_pin: [u8; 44], intent: PublicIntent) -> Self {
        Self {
            key,
            gateway_pin,
            intent,
        }
    }
    /// Borrow the one private key owner for an exact save retry.
    pub fn key(&self) -> &PrivateKeyMaterial {
        &self.key
    }
    /// Authenticated gateway pin; authenticity comes from the client finish owner.
    pub fn gateway_pin(&self) -> &[u8; 44] {
        &self.gateway_pin
    }
    /// Exact invitation and attempt for fresh pinned status.
    pub fn intent(&self) -> PublicIntent {
        self.intent
    }
    /// Transfer the seed into the native signing owner after bounded restoration.
    pub fn into_parts(self) -> (PrivateKeyMaterial, [u8; 44], PublicIntent) {
        (self.key, self.gateway_pin, self.intent)
    }
}
impl fmt::Debug for PendingEnrollment {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output
            .debug_struct("PendingEnrollment")
            .field("key", &self.key)
            .field("intent", &self.intent)
            .finish_non_exhaustive()
    }
}

/// Physical key publication meaning retained when audit acknowledgement fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayPublicationState {
    /// No canonical key-file publication occurred in this operation.
    NotPublished,
    /// The exact canonical key file is known to exist; audit acknowledgement failed.
    Published,
}

/// Redacted storage failure meaning, independent of diagnostic text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateStorageFailure {
    /// Unsafe identity, binding or exact-byte evidence was observed.
    UnsafeStorage,
    /// The storage operation could not be completed.
    Unavailable,
}
/// The native publication checkpoint, translated by the storage adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivatePublicationStep {
    /// Destination validation refused.
    ValidateDestination,
    /// Original directory binding refused.
    VerifyOriginBinding,
    /// Original reservation identity refused.
    ValidateReservation,
    /// Pre-rename flush failed.
    FlushBeforeRename,
    /// Native rename failed.
    Rename,
    /// Published file flush failed.
    FlushAfterRename,
    /// Published destination identity refused.
    ValidatePublishedDestination,
    /// Published directory binding refused.
    VerifyPublishedBinding,
    /// Directory acknowledgement failed.
    SyncDirectory,
}
/// Physical rename fact supplied by the storage owner, never authorization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivatePublicationEffect {
    /// Storage supplied no published object.
    NotPublished,
    /// Storage supplied the original published object.
    Published,
}
/// Finite publication evidence with independent cleanup and reconciliation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivatePublicationError {
    step: PrivatePublicationStep,
    primary: PrivateStorageFailure,
    cleanup: Option<PrivateStorageFailure>,
    effect: PrivatePublicationEffect,
    reconciliation: Option<PrivateStorageFailure>,
}
impl PrivatePublicationError {
    pub(crate) fn new(
        step: PrivatePublicationStep,
        primary: PrivateStorageFailure,
        cleanup: Option<PrivateStorageFailure>,
        effect: PrivatePublicationEffect,
        reconciliation: Option<PrivateStorageFailure>,
    ) -> Self {
        Self {
            step,
            primary,
            cleanup,
            effect,
            reconciliation,
        }
    }
    /// Original failed native checkpoint.
    pub fn step(&self) -> PrivatePublicationStep {
        self.step
    }
    /// Original redacted failure.
    pub fn primary(&self) -> PrivateStorageFailure {
        self.primary
    }
    /// Independent original reservation cleanup failure.
    pub fn cleanup(&self) -> Option<PrivateStorageFailure> {
        self.cleanup
    }
    /// Physical rename fact, not current product permission.
    pub fn effect(&self) -> PrivatePublicationEffect {
        self.effect
    }
    /// Failure of a subsequent live acknowledgement attempt.
    pub fn reconciliation(&self) -> Option<PrivateStorageFailure> {
        self.reconciliation
    }
}

/// Typed local storage outcomes without secret bytes or native diagnostic strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateStateError {
    /// Preserve known key publication separately from unavailable audit evidence.
    AuditUnavailable(GatewayPublicationState),
    /// Native publication failed, retaining physical and cleanup evidence.
    Publication(PrivatePublicationError),
    /// Audit-file failure retains its own effect separately from gateway-key publication.
    AuditPublication {
        /// Already owned gateway-key publication fact.
        key: GatewayPublicationState,
        /// Audit-file native publication failure.
        failure: PrivatePublicationError,
    },
    /// Unsafe or malformed storage was observed; no repair was attempted.
    Corrupt,
    /// An existing state differs from the expected key or correlation.
    Conflict,
    /// Another physical owner retains the lifetime private-store lock.
    Locked,
    /// An operation failed before publication was known to occur.
    Unavailable,
    /// Publication occurred but its acknowledgement could not be reconciled.
    Uncertain,
}
impl fmt::Display for PrivateStateError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(output, "{self:?}")
    }
}
impl Error for PrivateStateError {}

/// Gateway key persistence. First creation requires canonical bootstrap admission
/// at composition; this port does not infer absence of enrollment history.
pub trait GatewayKeyStore: Send + Sync {
    /// Restore the existing seed, or report absence without creating a key.
    fn restore_gateway_key(
        &self,
        gateway: &AudienceId,
        clock: &dyn Clock,
    ) -> Result<Option<PrivateKeyMaterial>, PrivateStateError>;
    /// Exclusively publish a seed; exact retry is idempotent, changed seed conflicts.
    fn save_gateway_key(
        &self,
        key: &PrivateKeyMaterial,
        gateway: &AudienceId,
        clock: &dyn Clock,
    ) -> Result<(), PrivateStateError>;
}

/// Local client persistence injected into the pre-KE3 finish callback.
pub trait ClientPendingStore: Send + Sync {
    /// Restore the atomic seed/pin/public correlation; absence is explicit.
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError>;
    /// Publish or compare-and-swap exact prior metadata, preserving identity on retry.
    /// The calling application owns admission from a pinned terminal receipt.
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        gateway_pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError>;
}
