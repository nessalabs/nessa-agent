use super::*;
use crate::application::agent_execution::providers::CloseOutcome;
use std::{collections::VecDeque, sync::Mutex};

struct SequencedCleanup(Mutex<VecDeque<CleanupReport>>);

impl ProviderCleanup for SequencedCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.0
                .lock()
                .expect("sequenced cleanup lock")
                .pop_front()
                .expect("one cleanup result per retry")
        })
    }
}

#[tokio::test]
async fn restoration_recovery_keeps_its_cause_across_failed_and_successful_cleanup() {
    let cause = AgentError::Transport("restore spawn failed".into());
    let cleanup = Arc::new(SequencedCleanup(Mutex::new(VecDeque::from([
        CleanupReport::unconfirmed(AgentError::CleanupUncertain),
        CleanupReport::confirmed(CloseOutcome { forced: false }),
    ]))));
    let recovery = RestorationRecovery {
        cause: cause.clone(),
        cleanup,
    };

    let first = recovery.retry().await;
    assert!(matches!(
        first.resources(),
        ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain)
    ));
    assert_eq!(first.operation_failure(), Some(&cause));

    let second = recovery.retry().await;
    assert!(matches!(
        second.resources(),
        ResourceCleanup::Confirmed(CloseOutcome { forced: false })
    ));
    assert_eq!(second.operation_failure(), Some(&cause));
}
