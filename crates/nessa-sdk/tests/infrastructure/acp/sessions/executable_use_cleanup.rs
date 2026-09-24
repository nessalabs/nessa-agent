//! Confirmed physical cleanup and durable use release remain separate facts.
use super::*;
use crate::application::agent_execution::providers::{
    ExecutableUseError, ExecutableUseGuard, ResourceCleanup,
};
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct RetryRelease {
    attempts: Arc<AtomicUsize>,
}

#[cfg(unix)]
struct DurableMarkerGuard {
    marker: PathBuf,
    releases: Arc<AtomicUsize>,
}

#[cfg(unix)]
impl ExecutableUseGuard for DurableMarkerGuard {
    fn release(&mut self) -> Result<(), ExecutableUseError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        std::fs::remove_file(&self.marker)
            .map_err(|error| ExecutableUseError::new(error.to_string()))
    }
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

#[cfg(unix)]
#[test]
fn cancelling_the_final_cleanup_supervisor_does_not_release_durable_use_evidence() {
    use crate::infrastructure::process::{DirectoryCleanupStep, ProcessScope};
    use tokio::sync::oneshot;

    let marker_root = tempfile::tempdir().unwrap();
    let marker = marker_root.path().join("managed-use-generation");
    std::fs::write(&marker, b"unresolved").unwrap();
    let releases = Arc::new(AtomicUsize::new(0));
    let (cleanup_started, cleanup_started_rx) = oneshot::channel();
    let (_finish_cleanup, finish_cleanup_rx) = oneshot::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let (_root, config, _) =
            crate::infrastructure::acp::tests::profile_substitution::profile_setup();
        let mut failure = match ProcessScope::spawn_with_private_directory(|_| {
            tokio::process::Command::new("/nessa-test/no-such-runtime")
        }) {
            Ok(_) => panic!("the missing runtime unexpectedly started"),
            Err(failure) => failure,
        };
        failure.script_directory_cleanup(vec![DirectoryCleanupStep::new(
            cleanup_started,
            finish_cleanup_rx,
        )]);
        let (_, retained) = failure.into_parts();
        let cleanup = ProcessCleanup::retaining_directory(
            config,
            retained.expect("the failed spawn retained its directory"),
            Box::new(DurableMarkerGuard {
                marker: marker.clone(),
                releases: releases.clone(),
            }),
        );

        drop(cleanup);
        cleanup_started_rx
            .await
            .expect("the final supervisor began physical cleanup");
    });
    runtime.shutdown_background();

    assert_eq!(releases.load(Ordering::SeqCst), 0);
    assert!(
        marker.exists(),
        "runtime shutdown cancelled the final cleanup task before use release"
    );
}
