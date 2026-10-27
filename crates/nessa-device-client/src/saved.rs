//! Generated foreign snapshots consume the shared cache projection. The explicit
//! target is a saved-data selector, not authority to connect or execute commands.
use crate::{
    infrastructure::{Monotonic, Wall},
    serialization::{bounded_json, SerializationFailure},
};
use nessa_auth::application::ports::Clock as WallClock;
use nessa_client_core::retained::{ReadFailure, RetainedCache, RetainedTranscript};
use nessa_protocol::{
    clock::Clock,
    conversation::{domain::ConversationId, projection::MAX_VIEW_BYTES},
};
use nessa_sync::replication::domain::Scope;
use serde::Serialize;
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FormatResult},
    path::Path,
    sync::{Arc, Mutex, TryLockError},
};
use uuid::Uuid;

/// Saved cache or projection failure, independent of absence/deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Error)]
pub enum NativeViewError {
    /// A physical operation holds this cache handle.
    Busy,
    /// An earlier unexpected failure invalidated this handle's admission.
    DamagedHandle,
    /// Native cache could not be opened or read.
    Cache,
    /// A prior cache commit remains unconfirmed; reload durable evidence.
    CacheUncertain,
    /// Saved schema/data needs explicit recovery.
    CacheDamaged,
    /// Cache selected another scope or history.
    ResetRequired,
    /// Cache exceeded its published quota.
    Quota,
    /// Explicit target is invalid.
    InvalidTarget,
    /// Passive view could not be serialized within the protocol-owned bound.
    ViewTooLarge,
    /// Passive view serialization failed for another reason.
    Serialization,
}
impl Display for NativeViewError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FormatResult {
        write!(f, "{self:?}")
    }
}
impl Error for NativeViewError {}
fn cache_error(error: ReadFailure) -> NativeViewError {
    match error {
        ReadFailure::CacheUncertain => NativeViewError::CacheUncertain,
        ReadFailure::CacheDamaged => NativeViewError::CacheDamaged,
        ReadFailure::ResetRequired => NativeViewError::ResetRequired,
        ReadFailure::Quota => NativeViewError::Quota,
        ReadFailure::Protocol
        | ReadFailure::CursorAhead
        | ReadFailure::SubscriptionCapacity
        | ReadFailure::ConversationDeleted
        | ReadFailure::ConversationNotFound => NativeViewError::InvalidTarget,
        ReadFailure::Cache
        | ReadFailure::Refused
        | ReadFailure::Unreachable
        | ReadFailure::Away
        | ReadFailure::Stopped => NativeViewError::Cache,
    }
}

/// An immutable saved transcript's meaning, independently of current authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum NativeTranscriptState {
    /// No checkpoint is readable for the selected target.
    NotLoaded,
    /// An actual retained deletion fence exists.
    Deleted,
    /// The core projected its actual saved checkpoint and physical suffix.
    Retained,
}
/// Immutable scope from the core snapshot, independently of socket metadata.
#[derive(uniffi::Object)]
pub struct NativeScope {
    pub(crate) value: Scope,
}
#[uniffi::export]
impl NativeScope {
    /// Compare all six core scope identities, independently of endpoint strings.
    pub fn same_as(&self, other: Arc<Self>) -> bool {
        self.value == other.value
    }
    /// Saved receiver identity.
    pub fn receiver(&self) -> String {
        self.value.receiver().as_str().to_owned()
    }
    /// Saved gateway origin identity; a host address is not an origin.
    pub fn origin(&self) -> String {
        self.value.origin().as_str().to_owned()
    }
    /// Saved physical stream identity.
    pub fn stream(&self) -> String {
        self.value.stream().as_str().to_owned()
    }
    /// Saved source incarnation.
    pub fn incarnation(&self) -> String {
        self.value.incarnation().as_str().to_owned()
    }
    /// Saved physical schema identity.
    pub fn schema(&self) -> String {
        self.value.schema().as_str().to_owned()
    }
    /// Saved access epoch identity, preserved as exact text.
    pub fn access_epoch(&self) -> String {
        self.value.access_epoch().as_str().to_owned()
    }
}

/// An owned core snapshot. Getters cannot replace its progress/view pairing.
#[derive(uniffi::Object)]
pub struct NativeTranscript {
    pub(crate) value: RetainedTranscript,
}
#[uniffi::export]
impl NativeTranscript {
    /// Explicit not-loaded/deleted/retained state, not inferred from empty text.
    pub fn state(&self) -> NativeTranscriptState {
        match self.value {
            RetainedTranscript::NotLoaded => NativeTranscriptState::NotLoaded,
            RetainedTranscript::Deleted => NativeTranscriptState::Deleted,
            RetainedTranscript::Retained(_) => NativeTranscriptState::Retained,
        }
    }
    /// Actual scope from this owned progress/view snapshot.
    pub fn scope(&self) -> Option<Arc<NativeScope>> {
        match &self.value {
            RetainedTranscript::Retained(value) => Some(Arc::new(NativeScope {
                value: value.progress().scope().clone(),
            })),
            _ => None,
        }
    }
    /// Actual contiguous downloaded physical position, if a snapshot exists.
    pub fn downloaded(&self) -> Option<u64> {
        match &self.value {
            RetainedTranscript::Retained(value) => Some(value.progress().downloaded()),
            _ => None,
        }
    }
    /// Actual semantic apply position, separately from downloaded progress.
    pub fn applied(&self) -> Option<u64> {
        match &self.value {
            RetainedTranscript::Retained(value) => Some(value.progress().applied()),
            _ => None,
        }
    }
    /// Last contributing physical fact position.
    pub fn facts(&self) -> Option<u64> {
        match &self.value {
            RetainedTranscript::Retained(value) => Some(value.progress().facts()),
            _ => None,
        }
    }
    /// The durable generation from which this snapshot was projected.
    pub fn generation(&self) -> Option<u64> {
        match &self.value {
            RetainedTranscript::Retained(value) => Some(value.progress().generation()),
            _ => None,
        }
    }
    /// Existing passive ConversationView JSON; no partial bytes escape on failure.
    /// The `serialization_bound_does_not_return_partial_view` test exercises the bound.
    pub fn view_json(&self) -> Result<Option<String>, NativeViewError> {
        let RetainedTranscript::Retained(value) = &self.value else {
            return Ok(None);
        };
        serialize_view(value.view()).map(Some)
    }
}

/// Off-UI saved projection adapter. This is an explicit historical cache selector,
/// not an enrolled session or the phone's live repository. Session composition
/// owns receiver selection and revocation fencing in the subsequent driver.
#[derive(uniffi::Object)]
pub struct NativeSavedCache {
    cache: Mutex<RetainedCache>,
}
impl NativeSavedCache {
    pub(crate) fn with_clocks(
        path: &Path,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, NativeViewError> {
        Ok(Self {
            cache: Mutex::new(RetainedCache::open(path, wall, clock).map_err(cache_error)?),
        })
    }
}
#[uniffi::export]
impl NativeSavedCache {
    /// Open the existing retained cache in an already private directory. Call on
    /// a dedicated worker; this constructor performs blocking storage work.
    #[uniffi::constructor]
    pub fn open(path: String) -> Result<Self, NativeViewError> {
        Self::with_clocks(Path::new(&path), Arc::new(Wall), Arc::new(Monotonic::new()))
    }
    /// Project an explicit historical target through the existing shared fold.
    /// A concurrent operation returns Busy; no cache operation queue is created.
    pub fn transcript(
        &self,
        receiver: String,
        origin: String,
        conversation: String,
        revision: String,
    ) -> Result<Arc<NativeTranscript>, NativeViewError> {
        let mut cache = match self.cache.try_lock() {
            Ok(cache) => cache,
            Err(TryLockError::WouldBlock) => return Err(NativeViewError::Busy),
            Err(TryLockError::Poisoned(_)) => return Err(NativeViewError::DamagedHandle),
        };
        let conversation =
            ConversationId::new(&conversation).map_err(|_| NativeViewError::InvalidTarget)?;
        let revision = Uuid::parse_str(&revision).map_err(|_| NativeViewError::InvalidTarget)?;
        let value = cache
            .transcript_view(&receiver, &origin, &conversation, revision)
            .map_err(cache_error)?;
        Ok(Arc::new(NativeTranscript { value }))
    }
}

fn serialize_view<T: Serialize>(value: &T) -> Result<String, NativeViewError> {
    bounded_json(value, MAX_VIEW_BYTES).map_err(|failure| match failure {
        SerializationFailure::TooLarge => NativeViewError::ViewTooLarge,
        SerializationFailure::Invalid => NativeViewError::Serialization,
    })
}
#[cfg(test)]
#[path = "../tests/saved/projection.rs"]
mod tests;
