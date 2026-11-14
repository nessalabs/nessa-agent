use crate::github::domain::AccessToken;
use crate::github_relay::domain::{Binding, Event, Failure, Page};
#[async_trait::async_trait]
pub trait GitHubCredentials: Send + Sync {
    async fn token(&self) -> Result<AccessToken, Failure>;
    async fn identify(&self, repository: &str) -> Result<Binding, Failure>;
}
#[async_trait::async_trait]
pub trait RelaySource: Send + Sync {
    async fn page(&self, token: &str, binding: &Binding, after: i64) -> Result<Page, Failure>;
}
#[async_trait::async_trait]
pub trait RelayStore: Send + Sync {
    async fn bindings(&self) -> Result<Vec<Binding>, Failure>;
    async fn bind(&self, binding: &Binding, accept_gap: bool) -> Result<(), Failure>;
    async fn unbind(&self, binding: &Binding) -> Result<(), Failure>;
    async fn cursor(&self, binding: &Binding) -> Result<i64, Failure>;
    async fn commit(&self, binding: &Binding, after: i64, page: &Page) -> Result<(), Failure>;
    async fn gap(&self, binding: &Binding, through: i64) -> Result<(), Failure>;
    async fn events(&self, binding: &Binding) -> Result<Vec<Event>, Failure>;
}

#[async_trait::async_trait]
pub trait RelayNotifications: Send {
    async fn changed(&mut self) -> Result<(), Failure>;
}
#[async_trait::async_trait]
pub trait RelayLiveSource: RelaySource {
    async fn watch(&self, ticket: &str) -> Result<Box<dyn RelayNotifications>, Failure>;
}
#[async_trait::async_trait]
pub trait RelayWait: Send + Sync {
    async fn wait(&self, duration: std::time::Duration);
}
pub trait RelayReporter: Send + Sync {
    fn disconnected(&self, binding: &Binding, failure: &Failure);
    fn storage_failed(&self, failure: &Failure);
    fn task_failed(&self);
}

/// Returns the persisted key; missing keys for an encrypted inbox refuse access.
/// Implementations perform blocking OS work on the store's blocking thread.
pub trait InboxKeySource: Send + Sync {
    fn load(&self) -> Result<std::sync::Arc<nessa_local_database::DatabaseKey>, Failure>;
}
