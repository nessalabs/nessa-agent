//! Pending completion and provider uncertainty cannot establish physical release.
use super::*;
#[tokio::test]
async fn an_unacquired_preparation_requires_a_completed_opening_without_holds() {
    let opening = OpeningResources::default();
    assert!(!opening.released(Duration::from_secs(10)).await);
    assert!(!opening.can_retire());
    opening.complete(false, Duration::from_secs(10)).await;
    assert!(opening.released(Duration::from_secs(10)).await);
    assert!(opening.can_retire());
    let held = OpeningResources::default();
    held.complete(true, Duration::from_secs(10)).await;
    assert!(!held.released(Duration::from_secs(10)).await);
    assert!(!held.can_retire());
}

use crate::conversation::application::environment::{
    open_lease, EnvironmentFuture, LeaseHold, LeaseRelease, LeaseRequest,
};
use crate::conversation::infrastructure::in_process_environment;
use crate::conversation_test_support::{Provider, ProviderFactory, TestClock, DELETION_BUDGETS};
use nessa_sdk::application::agent_execution::{agents::AgentError, providers::AgentProvider};
use nessa_sdk::domain::agent_execution::leases::{
    AgentWork, LeaseCleanup, LeaseEndCause, SandboxProfile, SandboxProfiles,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReleaseFault {
    poll: bool,
    calls: Arc<AtomicUsize>,
}
impl LeaseHold for ReleaseFault {
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Ok(binding)
    }
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        None
    }
    fn end(&self, _: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        assert!(self.poll || !first, "release constructor fault");
        Box::pin(async move {
            assert!(!self.poll || !first, "release poll fault");
            LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
        })
    }
}
async fn test_lease(hold: Arc<dyn LeaseHold>) -> Arc<LiveLease> {
    let mut opening = open_lease(
        in_process_environment().as_ref(),
        LeaseRequest {
            work: AgentWork::new("claude", "test").unwrap(),
            sandbox: SandboxProfile::HarnessDefault,
            binding: SandboxProfiles::HARNESS_DEFAULT,
        },
        Arc::new(Provider::new(Arc::new(ProviderFactory::default()))),
        &TestClock,
    )
    .await;
    opening.hold = Some(hold);
    Arc::new(LiveLease::new(&opening))
}
#[tokio::test]
async fn preparation_release_fault_retains_the_real_owner_and_original_completion() {
    for poll in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let lease = test_lease(Arc::new(ReleaseFault {
            poll,
            calls: calls.clone(),
        }))
        .await;
        let opening = OpeningResources::default();
        opening.acquired(lease);
        opening.complete(false, DELETION_BUDGETS.stop).await;
        assert_eq!(opening.completion.get(), Some(&false));
        assert!(!opening.can_retire());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(opening.released(DELETION_BUDGETS.stop).await);
        assert!(opening.can_retire());
        assert!(opening.released(DELETION_BUDGETS.stop).await);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "confirmed release is reused"
        );
    }
}
#[tokio::test]
async fn successful_preparation_transfers_environment_ownership_without_retention() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = test_lease(Arc::new(ReleaseFault {
        poll: false,
        calls: calls.clone(),
    }))
    .await;
    let weak = Arc::downgrade(&lease);
    let opening = OpeningResources::default();
    opening.acquired(lease.clone());
    opening.transferred();
    opening.complete(true, DELETION_BUDGETS.stop).await;
    drop(lease);
    assert!(
        weak.upgrade().is_none(),
        "successful opening retained the transferred lease"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!opening.released(DELETION_BUDGETS.stop).await);
}
struct PendingRelease;
impl LeaseHold for PendingRelease {
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        Ok(binding)
    }
    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        None
    }
    fn end(&self, _: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn concurrent_cleanup_joiners_keep_each_original_wait_budget() {
    let lease = test_lease(Arc::new(PendingRelease)).await;
    let deadline = DELETION_BUDGETS.stop;
    let (first, second) = tokio::time::timeout(deadline + deadline / 2, async {
        tokio::join!(
            lease.release_preparation(deadline),
            lease.release_preparation(deadline)
        )
    })
    .await
    .expect("waiting for the cleanup owner consumed an additional cleanup budget");
    assert!(!first);
    assert!(!second);
    assert!(!lease.release_answered());
}

#[tokio::test]
async fn sdk_nonacquisition_requires_completed_exact_opening_and_never_survives_start() {
    let opening = OpeningResources::default();
    assert!(!opening.sdk_never_started_for(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = test_lease(Arc::new(ReleaseFault {
        poll: true,
        calls: calls.clone(),
    }))
    .await;
    let id = lease.id().clone();
    opening.acquired(lease);
    opening.complete(false, DELETION_BUDGETS.stop).await;
    assert!(opening.sdk_never_started_for(Some(&id)));
    assert!(
        !opening.can_retire(),
        "non-acquisition does not acknowledge unanswered environment release"
    );
    assert!(!opening.sdk_never_started_for(Some(&LeaseId::new("different-lease").unwrap())));
    assert!(opening.released(DELETION_BUDGETS.stop).await);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    opening.start_sdk_attachment();
    assert!(!opening.sdk_never_started_for(Some(&id)));
    assert!(!opening.sdk_never_started_for(None));
    assert!(opening.released(DELETION_BUDGETS.stop).await);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "SDK evidence is independent of the already joined environment owner"
    );
}
