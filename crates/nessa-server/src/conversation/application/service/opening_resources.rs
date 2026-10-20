//! The opening transaction retains its actual environment cleanup capability.
//! Completion describes SDK preparation separately from the acquired lease.
//! Root/child adapters keep this owner, never the successful Slot or Agent.
use super::LiveLease;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Default)]
pub(super) struct OpeningResources {
    pub(super) completion: OnceLock<bool>,
    lease: Mutex<Option<Arc<LiveLease>>>,
}
impl OpeningResources {
    pub(super) fn acquired(&self, lease: Arc<LiveLease>) {
        let mut retained = self.lease.lock().unwrap();
        assert!(
            retained.is_none(),
            "the previous opening lease was not transferred"
        );
        *retained = Some(lease);
    }
    pub(super) fn can_retire(&self) -> bool {
        self.completion.get() == Some(&false)
            && self
                .lease
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|lease| lease.release_answered())
    }
    pub(super) fn transferred(&self) {
        self.lease.lock().unwrap().take();
    }
    pub(super) async fn complete(&self, holds: bool, deadline: Duration) {
        // Even before a coordinator asks to close, attempt the acquired lease's
        // real release. An unanswered attempt retains the same retry owner.
        if !holds {
            self.release_environment(deadline).await;
        }
        let _ = self.completion.set(holds);
    }
    async fn release_environment(&self, deadline: Duration) -> bool {
        let lease = self.lease.lock().unwrap().clone();
        match lease {
            Some(lease) => lease.release_preparation(deadline).await,
            None => true,
        }
    }
    pub(super) async fn released(&self, deadline: Duration) -> bool {
        self.completion.get() == Some(&false) && self.release_environment(deadline).await
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/owned_preparation.rs"]
mod tests;
