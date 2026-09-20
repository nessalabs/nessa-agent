//! The Opencode profile against a handler speaking Opencode's own shapes: what
//! it selects, what it refuses to proceed without, and what survives
//! translation.
//!
//! The handler's frames were read off Opencode 1.18.31 by driving the installed
//! binary — the same one `nessa install-agent opencode` puts on the machine —
//! with an empty home and recording what it answered.
use super::support::*;
use crate::domain::agent_execution::tools::ToolContent;

#[tokio::test]
async fn a_session_is_opened_configured_and_prompted_through_the_shared_runtime() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("echo", 16);
    let mut opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened.session.capabilities().model().model_id(),
        "exact-fixture-model"
    );
    assert_eq!(
        opened
            .session
            .execute(prompt("first"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("first"))
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
    assert_gone(&root, "pid");
}

/// Opencode opens every session in `build` — it has no way to start in the
/// other — so the session this binding hands back has to be one it moved into
/// `plan` first. The handler asserts the order the selections arrive in; this
/// asserts that they arrive at all before anything can be prompted.
#[tokio::test]
async fn a_session_is_in_the_configured_mode_before_it_can_be_prompted() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("echo", 16);
    // Opening is what applies the configuration: a session that reached the
    // caller is a configured one, and the handler refuses the prompt otherwise.
    let opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("first"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
}

/// A mode change after the fact is the session leaving the policy it was opened
/// under. It arrives as an update rather than as an answer, so nothing else
/// would have questioned it.
#[tokio::test]
async fn a_session_that_leaves_its_mode_afterwards_fails_the_execution() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("left-the-mode", 16);
    let opened = binding.open(None).await.unwrap();
    let outcome = opened.session.execute(prompt("first")).await.into_result();
    assert!(outcome.is_err(), "{outcome:?}");
}

/// The version is the one `nessa install-agent opencode` pins, and the name is
/// the agent's own. An Opencode that Nessa did not install is one whose wire
/// behaviour nobody here has read.
#[tokio::test]
async fn an_opencode_this_profile_was_not_written_against_is_refused() {
    for mode in ["wrong-harness", "wrong-version"] {
        let _process_slot = process_test_slot().await;
        let (_root, binding) = test_opencode_binding(mode, 16);
        let opened = binding.open(None).await;
        assert!(opened.is_err(), "{mode} was accepted");
    }
}

#[tokio::test]
async fn a_session_is_refused_rather_than_run_half_configured() {
    for mode in ["model-not-offered", "model-refused", "mode-refused"] {
        let _process_slot = process_test_slot().await;
        let (_root, binding) = test_opencode_binding(mode, 16);
        let opened = binding.open(None).await;
        assert!(opened.is_err(), "{mode} opened a session anyway");
    }
}

/// OpenCode Zen rotates the free models it serves, so a provider reporting its
/// own configuration while this binding is still applying it is correct to name
/// the model it had rather than the one being asked for.
#[tokio::test]
async fn opencode_reporting_its_configuration_while_it_is_being_configured_is_not_a_failure() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("startup-update-configuring", 16);
    assert!(binding.open(None).await.is_ok());
}

/// Opencode reports tool calls in the protocol's own shape, so what this
/// profile adds is that they survive whole — the announcement and the update
/// that completes it are one call, not two.
#[tokio::test]
async fn a_tool_call_arrives_with_what_it_read() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("read-tool-call", 16);
    let mut opened = binding.open(None).await.unwrap();
    let running = start(&opened, "first").await;
    let mut contents = Vec::new();
    loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(update) => {
                if let Some(content) = update.content() {
                    contents.extend(content.iter().cloned());
                }
            }
            ExecutionUpdate::Finished(_) => break,
            _ => {}
        }
    }
    running.await.unwrap().unwrap();
    assert_eq!(contents, vec![ToolContent::text("fn main() {}".to_owned())]);
}

/// A permission request reaches the host naming the tool and carrying the
/// arguments it is asking about. Reviewing a title alone would record a decision
/// against "Edit" with nothing saying what would be edited.
#[tokio::test]
async fn an_edit_approval_reaches_the_host_with_what_it_would_change() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("edit-permission", 16);
    let mut opened = binding.open(None).await.unwrap();
    let running = start(&opened, "first").await;
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = next(&mut opened).await
    else {
        panic!("expected permission");
    };
    // The tool call's own title, and the arguments it would act on. A decision
    // recorded against a name with nothing under it is not a reviewed decision.
    assert_eq!(input.name, "Edit src/main.rs");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&input.arguments_json).unwrap(),
        serde_json::json!({"filePath": "src/main.rs", "newText": "fn main() {}"})
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
            attribution: attribution(),
            execution_id: ExecutionId::new("first").unwrap(),
            id,
            option_id: allow.clone(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"outcome":"selected","optionId":allow.as_str()})
    );
}
