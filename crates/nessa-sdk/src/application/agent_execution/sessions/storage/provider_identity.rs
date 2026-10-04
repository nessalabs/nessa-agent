//! Moving a saved session's restoration identity, with no provider involved.
#![deny(missing_docs)]

use super::{
    SessionChange, SessionLoadState, SessionSaveGeneration, SessionSaveUnit, SessionSnapshot,
    SessionStorageLease, StorageError,
};
use crate::application::agent_execution::providers::ProviderIdentity;
use crate::domain::agent_execution::sessions::SessionId;

/// The restoration identity a session was saved under, read through its
/// writer lease so that it can be moved to another one.
///
/// For a host whose way of computing a provider's identity changed while the
/// provider context it selects did not: it reads which identity the saved
/// history names, decides, and moves it with one
/// [`SessionChange::ProviderIdentity`]. The SDK owns that change's rule — the
/// fold refuses a move whose `before` is not the published identity, or that
/// does not change it — so this type only carries what was read to the save.
///
/// It holds no lease of its own: the caller keeps the
/// [`SessionStorageLease`] it read through, and passes the same one to
/// [`Self::move_to`], so nothing else can write the session in between.
#[derive(Debug)]
pub struct SavedProviderIdentity {
    snapshot: SessionSnapshot,
    binding: SessionSaveGeneration,
    state: SessionLoadState,
}

impl SavedProviderIdentity {
    /// Read what `lease` holds for the session `id`, checked as
    /// [`SessionSnapshot::load_saved`] checks it.
    ///
    /// Unlike that read, an unfinished save is not refused. Its prior
    /// publication is read with the original recovery binding, so that
    /// [`Self::move_to`] is the exact retry of a move interrupted after its
    /// unit became durable and before its completion; the writer refuses any
    /// other plan. [`Self::state`] says which was read. Nothing is written.
    ///
    /// Returns `None` when the session has no published history.
    ///
    /// # Errors
    /// A backend error from [`SessionStorageLease::load`];
    /// [`StorageError::Corrupt`] for history whose relationships do not hold;
    /// [`StorageError::IdentityMismatch`] for history saved under another
    /// session.
    pub async fn load(
        lease: &dyn SessionStorageLease,
        id: &SessionId,
    ) -> Result<Option<Self>, StorageError> {
        let loaded = lease.load().await.map_err(StorageError::bounded)?;
        let state = loaded.state();
        let (snapshot, binding) = loaded.into_checked(id)?;
        Ok(snapshot.map(|snapshot| Self {
            snapshot,
            binding,
            state,
        }))
    }

    /// The identity the published history restores under.
    pub fn provider(&self) -> &ProviderIdentity {
        &self.snapshot.provider
    }

    /// [`SessionLoadState::Unfinished`] when a save was left without its
    /// completion: [`Self::provider`] is then the prior publication's, and only
    /// the exact retry of that save is accepted.
    pub fn state(&self) -> SessionLoadState {
        self.state
    }

    /// Append `ProviderIdentity { before: self.provider(), after }` through
    /// `lease`, the lease this value was read through, and wait for the
    /// writer's acknowledgement. Nothing else in the snapshot changes, and no
    /// provider is opened.
    ///
    /// # Errors
    /// [`StorageError::Corrupt`] when the fold refuses the change (`after` is
    /// the identity already published) or when the writer refuses the save as
    /// not the exact retry of an unfinished one; a backend error when the save
    /// cannot be acknowledged. A failure is not proof that nothing was
    /// written: a later [`Self::load`] reads what stands.
    pub async fn move_to(
        self,
        lease: &dyn SessionStorageLease,
        after: ProviderIdentity,
    ) -> Result<(), StorageError> {
        let Self {
            snapshot, binding, ..
        } = self;
        let change = SessionChange::ProviderIdentity {
            before: snapshot.provider.clone(),
            after: after.clone(),
        };
        let moved = SessionSnapshot {
            provider: after,
            ..snapshot
        };
        let receipt = lease
            .save_changes(
                binding.clone(),
                moved,
                vec![SessionSaveUnit::new(vec![change])?],
            )
            .await?;
        receipt.next_for(&binding, 1).map(drop)
    }
}
