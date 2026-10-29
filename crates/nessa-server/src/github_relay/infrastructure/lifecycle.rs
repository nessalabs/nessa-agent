use crate::github_relay::{
    application::{RelayReporter, RelayWait},
    domain::{Binding, Failure},
};
pub struct TokioRelayWait;
#[async_trait::async_trait]
impl RelayWait for TokioRelayWait {
    async fn wait(&self, duration: std::time::Duration) {
        tokio::time::sleep(duration).await;
    }
}
pub struct TracingRelayReporter;
impl RelayReporter for TracingRelayReporter {
    fn disconnected(&self, binding: &Binding, failure: &Failure) {
        tracing::warn!(repository_id=%binding.repository_id(),user_id=%binding.user_id(),%failure,"GitHub relay disconnected; local cursor retained");
    }
    fn storage_failed(&self, failure: &Failure) {
        tracing::warn!(%failure,"GitHub inbox read failed");
    }
    fn task_failed(&self) {
        tracing::error!("GitHub relay task failed");
    }
}
