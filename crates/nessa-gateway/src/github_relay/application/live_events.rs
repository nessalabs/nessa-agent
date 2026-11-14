//! Explicit opt-in for the gateway's existing GitHub sign-in. The private
//! store fences imports with the same transaction that removes bindings.
use super::{GitHubCredentials, RelayStore};
use crate::github_relay::domain::{Binding, Failure};
use std::sync::Arc;

#[async_trait::async_trait]
pub trait ConnectedGithub: GitHubCredentials {
    fn user_id(&self) -> Option<String>;
    async fn repositories(&self) -> Result<Vec<Binding>, Failure>;
}
#[async_trait::async_trait]
pub trait LiveEventStore: RelayStore {
    /// Replace the complete opt-in atomically, retaining matching cursors.
    /// An empty selection disables and removes local events. Records name
    /// the authenticated caller, never a token.
    async fn select(
        &self,
        generation: u64,
        bindings: &[Binding],
        initiator: &str,
        accept_gaps: bool,
    ) -> Result<(), Failure>;
}
/// Owns the last commit boundary when an unlock may change during selection IO.
pub trait LiveSelectionAdmission: Send + Sync {
    fn commit_in_generation(
        &self,
        generation: u64,
        commit: &mut dyn FnMut() -> Result<(), Failure>,
    ) -> Result<(), Failure>;
}
pub struct LiveEvents {
    pub credentials: Arc<dyn ConnectedGithub>,
    pub store: Arc<dyn LiveEventStore>,
    changes: tokio::sync::Mutex<()>,
}
impl LiveEvents {
    pub fn new(credentials: Arc<dyn ConnectedGithub>, store: Arc<dyn LiveEventStore>) -> Self {
        Self {
            credentials,
            store,
            changes: tokio::sync::Mutex::new(()),
        }
    }
    pub async fn status(&self) -> Result<(bool, usize, usize), Failure> {
        let user = self.credentials.user_id();
        let bindings = self.store.bindings().await?;
        let mut count = 0;
        let mut gaps = 0;
        for binding in bindings {
            if Some(binding.user_id()) != user.as_deref() {
                continue;
            }
            count += 1;
            match self.store.cursor(&binding).await {
                Ok(_) => (),
                Err(Failure::Gap(_)) => gaps += 1,
                Err(error) => return Err(error),
            }
        }
        Ok((count > 0, count, gaps))
    }
    pub async fn set(
        &self,
        generation: u64,
        enabled: bool,
        initiator: &str,
        accept_gaps: bool,
    ) -> Result<(), Failure> {
        let _change = self.changes.lock().await;
        if !enabled {
            return self.store.select(generation, &[], initiator, false).await;
        }
        let user = self
            .credentials
            .user_id()
            .ok_or(Failure::CredentialsUnavailable)?;
        let bindings = self.credentials.repositories().await?;
        if bindings.is_empty() {
            return Err(Failure::Forbidden);
        }
        if bindings.len() > crate::github_relay::domain::contract::BINDINGS as usize {
            return Err(Failure::Capacity);
        }
        if self.credentials.user_id().as_deref() != Some(&user)
            || bindings.iter().any(|b| b.user_id() != user)
        {
            return Err(Failure::IdentityChanged);
        }
        self.store
            .select(generation, &bindings, initiator, accept_gaps)
            .await
    }
}
