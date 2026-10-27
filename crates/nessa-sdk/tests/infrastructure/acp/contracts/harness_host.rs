//! A binding whose harness a host starts (`AgentProvider::on_host`): the
//! same agent protocol, events, approvals and close as with a child of its
//! own, the harness started by the host from the binding's launch variables
//! alone, and released through the host's control.
use super::support::*;
use crate::infrastructure::harness_process::SupervisedHarness;
use std::{
    ffi::OsString,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

#[test]
fn a_workspace_on_a_host_is_not_resolved_on_this_machine() {
    let (root, config, _) = test_acp_configuration("success", 16);
    let local = root.path().to_path_buf();
    assert!(config.workspace_paths.resolve(&local).is_some());
    let hosted = config.on_host(local.clone());
    assert_eq!(
        hosted.workspace_paths.resolve(&local),
        None,
        "a path that exists here is not the host's path"
    );
}

/// A host that is this machine: it starts the fixture harness in its own
/// workspace, keeps what each launch asked for, and counts its cleanups.
struct LocalHost {
    workspace: PathBuf,
    mode: &'static str,
    launches: Mutex<Vec<BTreeMap<OsString, OsString>>>,
    cleanups: Arc<AtomicUsize>,
}

struct Counted {
    inner: Box<dyn HarnessControl>,
    cleanups: Arc<AtomicUsize>,
}
impl HarnessControl for Counted {
    fn cleanup(&mut self, grace: Duration, kill_timeout: Duration) -> HarnessCleanupFuture<'_> {
        self.cleanups.fetch_add(1, Ordering::SeqCst);
        self.inner.cleanup(grace, kill_timeout)
    }
}

impl HarnessHost for LocalHost {
    fn workspace(&self) -> &Path {
        &self.workspace
    }
    fn start(&self, launch: HarnessLaunch) -> Result<HarnessProcess, AgentError> {
        self.launches
            .lock()
            .unwrap()
            .push(launch.environment.clone());
        let opencode = self.mode == "opencode-echo";
        let fixture = if opencode {
            "opencode_acp_test_handler.py"
        } else {
            "claude_acp_test_handler.py"
        };
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/infrastructure/acp/contracts/fixtures")
                    .join(fixture),
            )
            .arg(if opencode { "echo" } else { self.mode })
            .current_dir(&self.workspace)
            .env_clear()
            .envs(launch.environment);
        if opencode {
            command
                .env("NESSA_REFUSED_OPENCODE_DATA_HOME", "/refused-data")
                .env("NESSA_REFUSED_OPENCODE_HOME", "/refused-home")
                .env("NESSA_REFUSED_OPENCODE_DB", "/refused-db")
                .env("OPENCODE_DB", self.workspace.join("sessions.sqlite"));
        }
        let process = if opencode {
            OpencodeAcpProvider::start_on_host(command)?
        } else {
            SupervisedHarness::start(command)?
        };
        Ok(HarnessProcess {
            control: Box::new(Counted {
                inner: process.control,
                cleanups: self.cleanups.clone(),
            }),
            ..process
        })
    }
}

#[tokio::test]
async fn a_binding_on_a_host_runs_there_with_the_same_events_approvals_and_close() {
    let _process_slot = process_test_slot().await;
    // The binding's own workspace is not where the harness runs.
    let (_here, binding) = test_acp_binding("permission", 16);
    let there = tempfile::tempdir().unwrap();
    let host = Arc::new(LocalHost {
        workspace: std::fs::canonicalize(there.path()).unwrap(),
        mode: "permission",
        launches: Mutex::new(Vec::new()),
        cleanups: Arc::new(AtomicUsize::new(0)),
    });
    let provider = binding.on_host(host.clone()).unwrap();
    let mut opened = provider
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let active = start(&opened, "write").await;
    assert!(matches!(next(&mut opened).await, ExecutionUpdate::Tool(_)));
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = next(&mut opened).await
    else {
        panic!("expected permission");
    };
    // The harness's own paths are the host's.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&input.arguments_json).unwrap()["file_path"],
        serde_json::json!(host.workspace.join("fixture.txt"))
    );
    let allow = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
        })
        .unwrap()
        .id()
        .clone();
    opened
        .session
        .answer_permission(PermissionAnswer {
            key: None,
            attribution: attribution(),
            execution_id: ExecutionId::new("write").unwrap(),
            id,
            option_id: allow,
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(active.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert!(host.workspace.join("fixture.txt").exists());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    // Released through the host, and gone.
    assert_eq!(host.cleanups.load(Ordering::SeqCst), 1);
    assert_gone(&there, "pid");
    // The launch carried the binding's own variables and nothing else: no
    // credential and no path of this machine.
    let launches = host.launches.lock().unwrap();
    assert_eq!(launches.len(), 1);
    for key in launches[0].keys() {
        assert!(
            ClaudeAcpProvider::LAUNCH_VARIABLES.contains(&key.to_str().unwrap()),
            "{key:?}"
        );
    }
}

#[tokio::test]
async fn opencode_runs_on_the_host_with_its_model_and_cleanup() {
    let _process_slot = process_test_slot().await;
    let (_here, binding) = test_opencode_binding("echo", 16);
    let there = tempfile::tempdir().unwrap();
    let host = Arc::new(LocalHost {
        workspace: std::fs::canonicalize(there.path()).unwrap(),
        mode: "opencode-echo",
        launches: Mutex::new(Vec::new()),
        cleanups: Arc::new(AtomicUsize::new(0)),
    });
    let provider = binding.on_host(host.clone()).unwrap();
    let mut opened = provider
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened.session.capabilities().model().model_id(),
        "exact-fixture-model"
    );
    assert_eq!(
        opened
            .session
            .execute(prompt("hosted"), Arc::new(|| Ok(())))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("hosted"))
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert!(host.cleanups.load(Ordering::SeqCst) > 0);
    assert!(host
        .launches
        .lock()
        .unwrap()
        .iter()
        .all(|launch| launch.is_empty()));
    assert_gone(&there, "pid");
}
