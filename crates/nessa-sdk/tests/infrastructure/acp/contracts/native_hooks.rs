//! A planted command hook runs when the launch omits suppression.
//!
//! ```text
//! switch on -> fixture sees no suppression request
//!            -> fixture runs hook_cmd.py with a PreToolUse payload
//!            -> log line is the hook-behavior record shape
//!            -> negotiated suppression fact stays what the profile already reports
//! ```
//!
//! Arrows are the order of one open. The fixture is not the real harness. It
//! runs the planted command only in `native-hooks` mode, and only after the
//! suppression keys are absent. Every other mode still requires those keys, so
//! a default launch that stops sending them fails the existing suppression
//! tests. A switch that still sends them fails the tests here.
//!
//! OpenCode has no switch and is not covered here.

use super::support::*;
use serde_json::json;
use std::path::{Path, PathBuf};

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
            .execute(prompt("with native hooks allowed"))
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
    assert!(capabilities.native_hooks());
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
async fn codex_native_hook_runs_when_the_launch_omits_suppression() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_codex_binding("native-hooks", 16);
    let log = plant(root.path());
    let binding = binding.with_native_hooks(NativeHooksSwitch::on());
    let capabilities = open_and_prompt(&binding).await;
    assert!(capabilities.negotiated());
    assert!(capabilities.native_hooks());
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
