//! What the gateway's pinned enrollment status means for this device's cache.
//!
//! ```text
//! pinned status --> Active   --> read with its receiver and current epoch
//!               --> Terminal --> client ends the enrollment record
//!                                --> PurgeBeforeEnd: purge the receiver first
//!               --> anything else, or no answer --> keep the cache, no read
//! ```
//! Arrows are decisions and calls. Only a Terminal status read over the
//! strictly pinned gateway key ends the device's issued credential record, and
//! the enrollment client does that itself. `PurgeBeforeEnd` is the record store it
//! is given: it purges the receiver's cached rows, with their receipt, before
//! the record may go, so a device never forgets a receiver whose data it still
//! holds. A refusal, a timeout, a closed connection or an unreadable status
//! ends nothing and purges nothing (design rows PC2–PC5).
use super::{CacheError, GatewayError, HeldFileRefusal, SubscriptionRefusal};
use nessa_auth::{
    application::pairing::{
        ClientPendingStore, DeviceCredential, PendingEnrollment, PrivateKeyMaterial,
        PrivateStateError,
    },
    domain::{pairing::PublicIntent, CredentialId, ResourceId},
};
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_protocol::product_contract::generated::{CatalogueReadErrorCode, RecordReadErrorCode};
use nessa_protocol::product_contract::ConnectionOutcome;
use nessa_sync::replication::domain::Id;
use std::sync::{Arc, Mutex, PoisonError};

/// The device's enrollment as the gateway reported it on a pinned connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinnedStatus {
    /// Readable: the receiver the credential is paired with and its epoch now.
    Active {
        /// Receiver named by the pinned gateway status.
        receiver: Id,
        /// Access epoch observed in that status.
        access_epoch: u64,
    },
    /// The enrollment ended; the client has ended the record, purging first.
    Terminal,
    /// Any phase before Active; nothing to read and nothing to purge.
    NotActive,
}

impl PinnedStatus {
    /// Interpret a status from the existing pinned channel owner for retained
    /// reads. This classification grants no authority; the gateway authorizes
    /// each read. Invalid sync receiver identity returns `None`.
    pub fn from_native(status: &NativePairingStatus) -> Option<Self> {
        Some(match status {
            NativePairingStatus::Active {
                receiver,
                access_epoch,
                ..
            } => Self::Active {
                receiver: Id::new(receiver.as_str()).ok()?,
                access_epoch: *access_epoch,
            },
            NativePairingStatus::Terminal { .. } => Self::Terminal,
            NativePairingStatus::Pending(_)
            | NativePairingStatus::Unclaimed { .. }
            | NativePairingStatus::Claimed(_)
            | NativePairingStatus::Approved(_)
            | NativePairingStatus::Staging(_) => Self::NotActive,
        })
    }
}

/// Original durable purge evidence; returning it again does not repeat it.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PurgeReceipt {
    pub(crate) receiver: ResourceId,
    /// Transcript streams, raw records and catalogue entries deleted.
    pub(crate) transcripts: u64,
    pub(crate) records: u64,
    pub(crate) catalogue_entries: u64,
    pub(crate) observed_at_ms: u64,
}

/// The cache owner deletes one receiver's data with its receipt, atomically.
pub(crate) trait CachePurges {
    /// Delete every cached row of `receiver` and keep a receipt naming the
    /// authenticated Terminal enrollment status as cause and initiator, what
    /// was deleted and when. The receipt fences the receiver: later applies
    /// refuse. A repeated purge returns the original receipt.
    fn purge_receiver(&mut self, receiver: &ResourceId) -> Result<PurgeReceipt, CacheError>;
}

/// Opens the cache a purge runs on, only when one is needed.
pub(crate) type OpenPurges =
    Box<dyn Fn() -> Result<Box<dyn CachePurges + Send>, CacheError> + Send + Sync>;

/// The device's enrollment record store, purging the receiver's cache before
/// the enrollment client may remove its record (`purge_precedes_record_removal`).
pub(crate) struct PurgeBeforeEnd {
    store: Arc<dyn ClientPendingStore>,
    open: OpenPurges,
    purged: Mutex<Option<Result<PurgeReceipt, CacheError>>>,
}
impl PurgeBeforeEnd {
    /// Retain one private-state owner; derive the purge receiver from the
    /// issued credential matching the authenticated end's expected intent.
    pub(crate) fn new(store: Arc<dyn ClientPendingStore>, open: OpenPurges) -> Self {
        Self {
            store,
            open,
            purged: Mutex::new(None),
        }
    }
    /// The purge an ended enrollment ran, if it ran: its receipt, or the cache
    /// failure that kept the record.
    pub(crate) fn purge(&self) -> Option<Result<PurgeReceipt, CacheError>> {
        self.purged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// Called only inside an admitted physical status operation.
    pub(crate) fn begin_status(&self) {
        *self.purged.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
    /// Transfer this operation's effect before its physical permit releases.
    pub(crate) fn finish_status(&self) -> Option<Result<PurgeReceipt, CacheError>> {
        self.purged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
impl ClientPendingStore for PurgeBeforeEnd {
    fn enrollment_key(&self) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
        self.store.enrollment_key()
    }
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.store.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.store.load_credential()
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.store.save_credential(credential, receiver, expected)
    }
    /// The client ends a record on an authenticated Terminal status, and ends
    /// a pending one on an authenticated Unclaimed status. Only an issued
    /// credential can have cached data, and the gateway never reports
    /// Unclaimed for an attempt that was claimed, so only ending a credential
    /// record purges (`only_ending_an_issued_credential_purges`). A purge that
    /// fails keeps the record, so the next Terminal status purges again.
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        let Some(saved) = self.store.load_credential()? else {
            return self.store.end_enrollment(expected);
        };
        if saved.intent() != expected {
            return Err(PrivateStateError::Conflict);
        }
        let purged = (self.open)().and_then(|mut cache| cache.purge_receiver(saved.receiver()));
        let refused = purged.is_err();
        *self.purged.lock().unwrap_or_else(PoisonError::into_inner) = Some(purged);
        if refused {
            return Err(PrivateStateError::Unavailable);
        }
        self.store.end_enrollment(expected)
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        gateway_pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.store.save_pending(key, gateway_pin, intent, expected)
    }
}

/// Whether a read failure is the gateway refusing this device's authority, so
/// the pinned status is asked again before anything else (design row PC5).
/// A refused `openProduct` counts: a revoked key is its usual cause (row
/// PR5), and the status read is what tells that cause from a full pool.
/// An authentication reply or close counts only when the shared connection
/// projection is a terminal revoke.
/// Plain EOF, a retryable close, `server_shutdown`, and `protocol_incompatible`
/// do not. The refusal itself never purges.
pub(crate) fn asks_status(error: GatewayError) -> bool {
    if let Some(outcome) = error.connection_outcome() {
        return outcome == ConnectionOutcome::TerminalRevoke;
    }
    matches!(
        error,
        GatewayError::ProductRefused
            | GatewayError::Record(
                RecordReadErrorCode::Unauthorized
                    | RecordReadErrorCode::Forbidden
                    | RecordReadErrorCode::WrongReceiver
                    | RecordReadErrorCode::StaleEpoch
            )
            | GatewayError::Catalogue(
                CatalogueReadErrorCode::Unauthorized
                    | CatalogueReadErrorCode::Forbidden
                    | CatalogueReadErrorCode::WrongReceiver
                    | CatalogueReadErrorCode::StaleEpoch
            )
            | GatewayError::HeldFile(HeldFileRefusal::Unauthorized | HeldFileRefusal::Forbidden)
            | GatewayError::Subscription(
                SubscriptionRefusal::Unauthorized | SubscriptionRefusal::Forbidden
            )
    )
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/application/device.rs"]
mod tests;
