//! Confirmed physical cleanup and durable use release remain separate facts.
use super::*;
use crate::application::agent_execution::providers::{
    ExecutableUseError, ExecutableUseGuard, ExecutableUseSnapshot, ResourceCleanup,
};
use crate::domain::agent_execution::permissions::PermissionOfferPolicy;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
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

fn cleanup_config() -> (tempfile::TempDir, AcpConfig) {
    let root = tempfile::tempdir().unwrap();
    let config = AcpConfig {
        executable: ExecutableUseSnapshot::unmanaged(root.path().join("unused-test-runtime")),
        arguments: Vec::new(),
        environment: BTreeMap::new(),
        credential_environment: BTreeMap::new(),
        workspace: root.path().to_owned(),
        tools_enabled: false,
        mcp_servers: Vec::new(),
        permissions: PermissionOfferPolicy::once_only(),
        launch_timeout: Duration::from_secs(1),
        startup_timeout: Duration::from_secs(1),
        execution_timeout: None,
        shutdown_grace: Duration::from_millis(10),
        kill_timeout: Duration::from_secs(1),
        event_capacity: 1,
        max_frame_bytes: 1024,
        max_incoming_frame_bytes: 1024,
        images: None,
        clock: Arc::new(crate::infrastructure::clock::RuntimeClock::new()),
    };
    assert!(config.executable.executable().is_absolute());
    assert!(config.workspace.is_absolute());
    assert!([config.shutdown_grace, config.kill_timeout,]
        .into_iter()
        .all(|duration| {
            !duration.is_zero() && tokio::time::Instant::now().checked_add(duration).is_some()
        }));
    #[cfg(unix)]
    config.validate().unwrap();
    (root, config)
}

#[tokio::test]
async fn release_acknowledgement_retries_without_repeating_confirmed_physical_cleanup() {
    let (_root, config) = cleanup_config();
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
        &ResourceCleanup::ReleasePending {
            physical: CloseOutcome { forced: false },
            failure: AgentError::Configuration(
                "executable use release acknowledgement failed: release journal acknowledgement failed"
                    .into()
            ),
        }
    );
    assert_eq!(
        first.physical_outcome(),
        Some(CloseOutcome { forced: false })
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let second = cleanup.retry_cleanup().await;
    assert_eq!(
        second.resources(),
        &ResourceCleanup::Confirmed(CloseOutcome { forced: false })
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn missing_spawn_transfer_never_acknowledges_executable_use_release() {
    let (_root, config) = cleanup_config();
    let attempts = Arc::new(AtomicUsize::new(0));
    let cleanup = ProcessCleanup::new(
        config,
        Box::new(RetryRelease {
            attempts: attempts.clone(),
        }),
    );

    let report = cleanup.retry_cleanup().await;
    assert_eq!(
        report.resources(),
        &ResourceCleanup::Unconfirmed(AgentError::CleanupUncertain)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 0);

    drop(cleanup);
    tokio::task::yield_now().await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "an untransferred spawned-process state cannot authorize release"
    );
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
        let (_root, config) = cleanup_config();
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
