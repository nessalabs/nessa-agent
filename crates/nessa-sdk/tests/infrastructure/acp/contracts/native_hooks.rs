//! A planted command hook runs when the launch omits suppression.
//!
//! ```text
//! switch on -> fixture sees no suppression request
//!            -> fixture runs hook_cmd.py with a PreToolUse payload
//!            -> log line is the hook-behavior record shape
//!            -> negotiated suppression fact stays what the profile already reports
//! switch off after that open -> the same conversation's next process start
//!                             -> fixture sees the suppression request
//! ```
//!
//! Arrows are the order of one open, and of one later start of that same
//! provider session. The fixture is not the real harness. It runs the planted
//! command only in `native-hooks` mode, and only after the suppression keys
//! are absent. `hooks-record` accepts either shape and writes what that
//! process start sent. Every other mode still requires the suppression keys,
//! so a default launch that stops sending them fails the existing suppression
//! tests. A switch that still sends them fails the tests here.
//!
//! OpenCode has no switch and is not covered here.

use super::support::*;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

fn plant(root: &Path) -> PathBuf {
    let log = root.join("native-hook.jsonl");
    let sentinel = root.join("sentinel");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../verification/hook-behavior/hook_cmd.py");
    let spec = json!({
        "script": script,
        "argv": ["--source", "project", &log, "record", &sentinel],
        "payload": {
            "hook_event_name": "PreToolUse",
            "tool_name": "Read",
            "tool_input": {"file_path": "NOTES.txt"},
            "source": "startup"
        }
    });
    std::fs::write(root.join("native-hook.json"), spec.to_string()).unwrap();
    log
}

fn recorded(log: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(log).expect("the planted hook wrote a log");
    serde_json::from_str(text.lines().next().expect("the log has a record"))
        .expect("the log line is JSON")
}

async fn open_and_prompt(binding: &impl AgentProvider) -> OperationCapabilities {
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .expect("allowing native hooks must not refuse the open");
    let capabilities = opened.session.operation_capabilities();
    assert_eq!(
        opened
            .session
            .execute(prompt("with native hooks allowed"), Arc::new(|| Ok(())))
            .await
            .into_result()
            .expect("a prompt must run while native hooks are allowed"),
        ExecutionOutcome::Completed
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    capabilities
}

#[tokio::test]
async fn claude_native_hook_runs_when_the_launch_omits_suppression() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("native-hooks", 16);
    let log = plant(root.path());
    let binding = binding.with_native_hooks(NativeHooksSwitch::on());
    let capabilities = open_and_prompt(&binding).await;
    assert!(capabilities.negotiated());
    assert!(capabilities.native_hooks_allowed());
    assert_eq!(
        capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unknown
    );
    let record = recorded(&log);
    assert_eq!(record["event"], "PreToolUse");
    assert_eq!(record["tool_name"], "Read");
    assert_eq!(record["hook_source"], "project");
    assert_eq!(record["source"], "startup");
    assert_eq!(record["mode"], "record");
    assert_eq!(record["exit_code"], 0);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_direct_open_records_no_attachment_as_its_cause() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (_root, binding) = test_acp_binding_with_audit("hooks-record", 16, audit.clone());
    let binding = binding.with_native_hooks(NativeHooksSwitch::on());
    open_and_prompt(&binding).await;
    let causes: Vec<_> = audit
        .launches
        .lock()
        .unwrap()
        .iter()
        .map(|launch| launch.cause())
        .collect();
    assert_eq!(causes, [NativeHooksLaunchCause::DirectOpen]);
}

fn hooks_launch(root: &Path, pid: i32) -> serde_json::Value {
    let path = root.join(format!("hooks-launch-{pid}"));
    serde_json::from_str(
        &std::fs::read_to_string(path).expect("the launch recorded its hooks request"),
    )
    .expect("the hooks record is JSON")
}

fn launch_pids(root: &Path) -> Vec<i32> {
    serde_json::from_str(&std::fs::read_to_string(root.join("launches")).unwrap()).unwrap()
}

/// Open with hooks on, turn the switch off, and relaunch the same provider session.
async fn relaunch_after_turning_hooks_off(
    binding: impl AgentProvider,
    flag: &AtomicBool,
) -> OpenedProviderSession {
    let (_, _, control) = ProviderOpenRequest::without_startup_control(None).into_parts();
    let request = ProviderOpenRequest::new(
        crate::domain::agent_execution::sessions::SessionId::new("conversation").unwrap(),
        None,
        control,
    );
    let opened = binding
        .open(request)
        .await
        .expect("hooks on must not refuse the open");
    assert!(opened
        .session
        .operation_capabilities()
        .native_hooks_allowed());
    flag.store(false, Ordering::Relaxed);
    // The process already running keeps the choice it started with.
    assert!(opened
        .session
        .operation_capabilities()
        .native_hooks_allowed());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("again"), Arc::new(|| Ok(())))
            .await
            .into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    assert!(!opened
        .session
        .operation_capabilities()
        .native_hooks_allowed());
    opened
}

#[tokio::test]
async fn turning_hooks_off_suppresses_an_existing_claude_conversations_next_launch() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_acp_binding_with_audit("hooks-record", 16, audit.clone());
    let flag = Arc::new(AtomicBool::new(true));
    let binding = binding.with_native_hooks(NativeHooksSwitch::from_shared(flag.clone()));
    let opened = relaunch_after_turning_hooks_off(binding, &flag).await;
    let pids = launch_pids(root.path());
    assert_eq!(pids.len(), 2, "relaunched");
    let first = hooks_launch(root.path(), pids[0]);
    assert_eq!(first["settingSources"], json!(["user", "project", "local"]));
    assert!(first["disableAllHooks"].is_null());
    let second = hooks_launch(root.path(), pids[1]);
    assert_eq!(second["settingSources"], json!([]));
    assert_eq!(second["disableAllHooks"], true);
    // Each start says why it happened: the open, then the runtime's restart
    // of the same session, which resumes its saved context.
    let launches: Vec<_> = audit
        .launches
        .lock()
        .unwrap()
        .iter()
        .map(|launch| (launch.cause(), launch.restored(), launch.allowed()))
        .collect();
    assert_eq!(
        launches,
        [
            (NativeHooksLaunchCause::Open, false, true),
            (NativeHooksLaunchCause::Restart, true, false),
        ]
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn turning_hooks_off_suppresses_an_existing_codex_conversations_next_launch() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("hooks-record", 16);
    let flag = Arc::new(AtomicBool::new(true));
    let binding = binding.with_native_hooks(NativeHooksSwitch::from_shared(flag.clone()));
    let opened = relaunch_after_turning_hooks_off(binding, &flag).await;
    let pids = launch_pids(root.path());
    assert_eq!(pids.len(), 2, "relaunched");
    let first = hooks_launch(root.path(), pids[0]);
    assert!(first["features"].is_null());
    assert!(first["notify"].is_null());
    let second = hooks_launch(root.path(), pids[1]);
    assert_eq!(second["features"], json!({"hooks": false}));
    assert_eq!(second["notify"], json!([]));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn codex_native_hook_runs_when_the_launch_omits_suppression() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("native-hooks", 16);
    let log = plant(root.path());
    let binding = binding.with_native_hooks(NativeHooksSwitch::on());
    let capabilities = open_and_prompt(&binding).await;
    assert!(capabilities.negotiated());
    assert!(capabilities.native_hooks_allowed());
    assert_eq!(
        capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unsupported
    );
    let record = recorded(&log);
    assert_eq!(record["event"], "PreToolUse");
    assert_eq!(record["tool_name"], "Read");
    assert_eq!(record["hook_source"], "project");
    assert_eq!(record["source"], "startup");
    assert_eq!(record["mode"], "record");
    assert_eq!(record["exit_code"], 0);
    assert_gone(&root, "pid");
}
