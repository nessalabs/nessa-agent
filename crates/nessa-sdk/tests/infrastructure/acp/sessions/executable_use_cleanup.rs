//! Confirmed physical cleanup and durable use release remain separate facts.
use super::*;
use crate::{
    application::agent_execution::providers::ResourceCleanup,
    infrastructure::acp::sessions::{ExecutableUseError, ExecutableUseGuard},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct RetryRelease {
    attempts: Arc<AtomicUsize>,
}

impl ExecutableUseGuard for RetryRelease {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(ExecutableUseError::new(
                "release journal acknowledgement failed",
            ))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn release_acknowledgement_retries_without_repeating_confirmed_physical_cleanup() {
    let (_root, config, _) =
        crate::infrastructure::acp::tests::profile_substitution::profile_setup();
    let attempts = Arc::new(AtomicUsize::new(0));
    let cleanup = ProcessCleanup::retaining_use(
        config,
        Box::new(RetryRelease {
            attempts: attempts.clone(),
        }),
    );

    let first = cleanup.retry_cleanup().await;
    assert!(!first.is_confirmed());
    assert_eq!(
        first.resources(),
        &ResourceCleanup::Unconfirmed(AgentError::Configuration(
            "executable use release acknowledgement failed: release journal acknowledgement failed"
                .into()
        ))
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let second = cleanup.retry_cleanup().await;
    assert_eq!(
        second.resources(),
        &ResourceCleanup::Confirmed(CloseOutcome { forced: false })
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
