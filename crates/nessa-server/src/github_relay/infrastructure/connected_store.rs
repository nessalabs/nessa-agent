//! Followers see only bindings for the current sign-in. Selection/deletion
//! and cursor commits are serialized by SQLite's immediate transactions.
use crate::github_relay::{
    application::{ConnectedGithub, LiveEventStore, RelayStore},
    domain::{Binding, Event, Failure, Page},
};
use std::sync::Arc;
pub struct ConnectedRelayStore {
    store: Arc<dyn LiveEventStore>,
    account: Arc<dyn ConnectedGithub>,
}
impl ConnectedRelayStore {
    pub fn new(store: Arc<dyn LiveEventStore>, account: Arc<dyn ConnectedGithub>) -> Self {
        Self { store, account }
    }
    fn current(&self, binding: &Binding) -> Result<(), Failure> {
        if self.account.user_id().as_deref() == Some(binding.user_id()) {
            Ok(())
        } else {
            Err(Failure::IdentityChanged)
        }
    }
}
#[async_trait::async_trait]
impl RelayStore for ConnectedRelayStore {
    async fn bindings(&self) -> Result<Vec<Binding>, Failure> {
        let user = self.account.user_id();
        Ok(self
            .store
            .bindings()
            .await?
            .into_iter()
            .filter(|b| Some(b.user_id()) == user.as_deref())
            .collect())
    }
    async fn bind(&self, b: &Binding, a: bool) -> Result<(), Failure> {
        self.current(b)?;
        self.store.bind(b, a).await
    }
    async fn unbind(&self, b: &Binding) -> Result<(), Failure> {
        self.current(b)?;
        self.store.unbind(b).await
    }
    async fn cursor(&self, b: &Binding) -> Result<i64, Failure> {
        self.current(b)?;
        self.store.cursor(b).await
    }
    async fn commit(&self, b: &Binding, a: i64, p: &Page) -> Result<(), Failure> {
        self.current(b)?;
        self.store.commit(b, a, p).await
    }
    async fn gap(&self, b: &Binding, t: i64) -> Result<(), Failure> {
        self.current(b)?;
        self.store.gap(b, t).await
    }
    async fn events(&self, b: &Binding) -> Result<Vec<Event>, Failure> {
        self.current(b)?;
        self.store.events(b).await
    }
}
