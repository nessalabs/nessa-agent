//! The Opencode profile against a handler speaking Opencode's own shapes: what
//! it selects, what it refuses to proceed without, and what survives
//! translation.
//!
//! Not all of the handler's frames were recorded, and the difference is stated
//! in the handler's own docstring rather than glossed here. The `initialize`
//! result and the `session/new` shapes were read off Opencode 1.18.31 by
//! driving the binary with an empty home. The tool call, the permission request
//! and the mid-turn mode change were not: each only happens during a model
//! turn, and this environment's network policy does not allow OpenCode Zen's
//! host, so those are written from the ACP specification. What the tests below
//! prove about them is that this profile translates those shapes correctly —
//! not that Opencode sends them.
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

/// The version is the one this profile was written against, and the name is the
/// agent's own. An Opencode of another version is one whose wire behaviour
/// nobody here has read, and it is refused rather than driven on the hope that
/// nothing moved. Nothing yet ties that version to one anybody installs — see
/// the note on `VERSION`.
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

/// Opencode opens every session in `build` and offers no way to start in the
/// mode this binding wants, so until the selection lands, `build` is the honest
/// answer to what mode the session is in. Announcing it is Opencode being
/// truthful, and the session must survive it — the pair of this test and
/// `a_session_that_leaves_its_mode_afterwards_fails_the_execution` is the whole
/// rule: tolerated before the selection, refused after it.
#[tokio::test]
async fn opencode_naming_the_mode_it_opened_in_is_not_leaving_the_one_it_is_given() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("announces-start-mode", 16);
    assert!(binding.open(None).await.is_ok());
}

/// Tolerating the mode Opencode opens in is not tolerating any mode at all.
/// The window before the selection lands admits the two modes the session
/// offers and nothing else, so a session announcing a third is still refused —
/// otherwise "we have not configured it yet" would be a hole a session could
/// be put into any policy through.
#[tokio::test]
async fn a_mode_the_session_never_offered_is_refused_even_before_it_is_configured() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("announces-unknown-mode", 16);
    assert!(binding.open(None).await.is_err());
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

/// A permission request reaches the host naming what class of action it is and
/// carrying the arguments it would act on.
///
/// Named by its ACP kind, not by its title. The title here is "Edit
/// src/main.rs", which is the friendlier label and the wrong thing to record: a
/// title is display text Opencode composes, possibly out of what the model
/// supplied, and nothing makes it agree with `rawInput`. The kind is one of the
/// protocol's ten and the arguments say what is being acted on, which together
/// are a decision somebody can be held to.
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
    // The kind, though the frame also carries a title. A decision recorded
    // against a name with nothing under it is not a reviewed decision, and one
    // recorded against a name the request chose the wording of is not either.
    assert_eq!(input.name, "edit");
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

/// Opencode's `initialize` advertises image prompts, and this binding declines
/// to pass that on. The reason is not Opencode: the shared worker builds every
/// `session/prompt` as a single text block, so a session that declared images
/// would be offering a modality with no way to reach the process — a caller
/// that took the offer would have its picture silently left behind.
#[tokio::test]
async fn a_session_offers_only_the_modality_a_prompt_can_carry() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding_on_a_model_that_takes_images("echo", 16);
    let opened = binding.open(None).await.unwrap();
    let features = opened.session.capabilities().features();
    assert!(features.input().text());
    assert!(
        !features.input().image(),
        "the binding passed the model's image input through"
    );
    assert!(!features.output().image());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
