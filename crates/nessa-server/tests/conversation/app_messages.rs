//! An MCP App in its conversation (#390): `mcp.sendMessage` and
//! `mcp.updateModelContext` through the conversation service, one test per
//! row of the state tables on the issue and in
//! `docs/design/mcp-app-calls.md` ("An app in its conversation") — a
//! message sent as the person's turn and attributed to the app, asked about
//! the first time per mount, refused while a turn runs; a context held per
//! mount, carried once by the next message, replaced, cleared, bounded, and
//! let go of with its mount or its opening — and the audit each leaves.
use super::*;
use crate::app_call_test_support::{
    caller, Fixture, Hold, INSTANCE, OTHER_INSTANCE, SERVER, UI_TOOL,
};
use crate::conversation::application::app_reviews::APP_REVIEW_DEADLINE;
use crate::conversation::application::app_reviews::{ALLOW, DENY, MAX_HELD_CONTEXTS};
use crate::conversation::application::view::{
    ConversationMessageApp, ConversationMessageStatus, ConversationPermissionOrigin,
};
use crate::conversation::application::ConversationLimits;
use crate::conversation::application::{SubmissionMode, SubmittedImage, SubmittedMessage};
use nessa_auth::domain::PrincipalId;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, ObservationFailure, ObservationFailureCause, ProviderExecutionReply,
    ProviderSessionState,
};
use nessa_sdk::domain::agent_execution::prompts::{AppModelContext, MessageSender, UserMessage};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

fn refused(result: Result<impl std::fmt::Debug, ConversationError>) -> McpAppError {
    match result {
        Err(ConversationError::McpApp(error)) => error,
        other => panic!("expected an app refusal, got {other:?}"),
    }
}

/// The person behind `caller(action)`, by that request.
fn person_by(action: &str) -> McpAppInitiator {
    McpAppInitiator::Person {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        request_id: action.into(),
    }
}

/// The app, on behalf of the fixture's person.
fn the_app() -> McpAppInitiator {
    McpAppInitiator::App {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
    }
}

/// The app's turn, as the transcript names who wrote it.
fn written_by(fixture: &Fixture) -> ConversationMessageApp {
    ConversationMessageApp {
        execution_id: fixture.execution_id.clone(),
        tool_id: fixture.tool_id.clone(),
        server: SERVER.into(),
        tool: UI_TOOL.into(),
    }
}

impl Fixture {
    /// Wait until every turn has finished, nothing waits, and what the
    /// turns carried is settled.
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let view = self
                    .service
                    .read(self.id.clone(), caller("read"))
                    .await
                    .unwrap();
                if view.pending.is_empty()
                    && view
                        .messages
                        .iter()
                        .all(|message| message.status == ConversationMessageStatus::Completed)
                    && !self.service.apps_of(&self.id).carrying()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    /// The mount `instance` allowed to send messages: its first message,
    /// allowed, and finished.
    async fn allowed(&self, instance: &str) -> String {
        let (task, review) = self.held_message(instance, "first").await;
        self.answer(&review, ALLOW).await;
        let execution = task.await.unwrap().unwrap();
        self.idle().await;
        execution
    }

    /// The contexts the agent was given with `execution`, as text.
    async fn contexts_given(&self, execution: &str) -> Vec<Option<String>> {
        self.given(execution)
            .await
            .app_model_context()
            .iter()
            .map(|context| context.text().map(str::to_owned))
            .collect()
    }

    /// The person sends `execution` and it finishes.
    async fn person_turn(&self, execution: &str) {
        self.person_sends(execution, "and now?").await;
        self.idle().await;
    }
}

// --- mcp.sendMessage ------------------------------------------------------

#[tokio::test]
async fn m2_a_message_from_no_app_or_for_another_server_is_refused_on_record() {
    let fixture = Fixture::new().await;
    let mut unknown = fixture.app(INSTANCE);
    unknown.tool_id = "not-a-tool-call".into();
    let result = fixture
        .service
        .send_app_message(
            fixture.id.clone(),
            caller("app-message"),
            McpAppMessage {
                app: unknown,
                server: SERVER.into(),
                text: "hello".into(),
            },
        )
        .await;
    assert_eq!(refused(result), McpAppError::AppUnknown);
    let result = fixture
        .service
        .send_app_message(
            fixture.id.clone(),
            caller("app-message"),
            McpAppMessage {
                app: fixture.app(INSTANCE),
                server: "files".into(),
                text: "hello".into(),
            },
        )
        .await;
    assert_eq!(refused(result), McpAppError::ServerMismatch);
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Refused(McpAppCode::AppUnknown),
            McpAppAuditPhase::Refused(McpAppCode::ServerMismatch),
        ]
    );
    assert!(fixture.app_reviews().await.is_empty());
}

#[tokio::test]
async fn m3_m4_a_blank_message_or_one_past_the_input_bound_is_refused() {
    let fixture = Fixture::new().await;
    assert!(matches!(
        fixture.send_message(INSTANCE, " \n\t").await,
        Err(ConversationError::InvalidInput)
    ));
    let bound = ConversationLimits::default().max_input_bytes;
    // Counted in UTF-8 bytes: a multibyte character one byte over is over.
    let over = format!("{}é", "x".repeat(bound - 1));
    assert_eq!(
        refused(fixture.send_message(INSTANCE, &over).await),
        McpAppError::RequestTooLarge
    );
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Refused(McpAppCode::InvalidRequest),
            McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge),
        ]
    );
    // Exactly at the bound is a message: it asks to be sent.
    let (task, review) = fixture.held_message(INSTANCE, &"x".repeat(bound)).await;
    fixture.answer(&review, ALLOW).await;
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn m5_a_released_mount_sends_nothing() {
    let fixture = Fixture::new().await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(
        refused(fixture.send_message(INSTANCE, "hello").await),
        McpAppError::Cancelled
    );
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].phase,
        McpAppAuditPhase::Refused(McpAppCode::Cancelled)
    );
    assert_eq!(records[0].initiator, McpAppInitiator::System);
    assert!(fixture.app_reviews().await.is_empty());
}

#[tokio::test]
async fn m6_m7_m12_the_first_message_asks_and_allowed_lands_as_the_persons_turn_written_by_the_app()
{
    let fixture = Fixture::new().await;
    let (task, review) = fixture
        .held_message(INSTANCE, "Plot May next to April")
        .await;
    // The review says what is asked, by whom, and shows exactly what is sent.
    assert_eq!(
        review.origin,
        ConversationPermissionOrigin::App {
            server: SERVER.into(),
            tool: UI_TOOL.into(),
        }
    );
    assert_eq!(
        review.title,
        format!("The {UI_TOOL} app on {SERVER} asks to send a message as you")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&review.arguments_json).unwrap(),
        serde_json::json!({"text": "Plot May next to April"})
    );
    assert_eq!(review.execution_id, fixture.execution_id);
    assert_eq!(review.tool_id, fixture.tool_id);
    // Nothing reaches the agent before the person answers.
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);

    fixture.answer(&review, ALLOW).await;
    let execution = task.await.unwrap().unwrap();

    // The agent was given the person's turn, written by the app.
    let given = fixture.given(&execution).await;
    assert_eq!(given.text_str(), "Plot May next to April");
    let MessageSender::App(app) = given.sender() else {
        panic!("written by the app: {given:?}");
    };
    assert_eq!(app.execution_id().as_str(), fixture.execution_id);
    assert_eq!(app.tool_id().as_str(), fixture.tool_id);
    assert_eq!((app.tool().server(), app.tool().tool()), (SERVER, UI_TOOL));

    // The transcript shows it as the app's, and the person's own as theirs.
    fixture.idle().await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == execution)
        .unwrap();
    assert_eq!(message.user_text, "Plot May next to April");
    assert_eq!(message.app, Some(written_by(&fixture)));
    assert_eq!(view.messages[0].app, None);

    // Every step on record: asked by the app, allowed by the person, sent.
    let records = fixture.audit.records.lock().unwrap().clone();
    let steps: Vec<_> = records
        .iter()
        .map(|record| (record.phase.clone(), record.initiator.clone()))
        .collect();
    assert_eq!(
        steps,
        [
            (
                McpAppAuditPhase::ApprovalRequested {
                    permission_id: review.permission_id.clone()
                },
                the_app()
            ),
            (
                McpAppAuditPhase::Approved {
                    permission_id: review.permission_id.clone(),
                    with: None,
                },
                person_by("answer")
            ),
            (
                McpAppAuditPhase::MessageSent {
                    execution_id: execution.clone()
                },
                the_app()
            ),
        ]
    );
    assert!(records.iter().all(|record| record.ask
        == McpAppAsk::SendMessage {
            server: SERVER.into()
        }
        && record.call_id == records[0].call_id));
}

#[tokio::test]
async fn m9_a_mount_allowed_sends_again_without_asking_and_another_mount_asks() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    fixture.audit.records.lock().unwrap().clear();

    let execution = fixture.send_message(INSTANCE, "again").await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), "again");
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::MessageSent {
                execution_id: execution
            },
        ]
    );
    assert!(fixture.app_reviews().await.is_empty());
    fixture.idle().await;

    // Allowing one mount is not allowing another of the same tool call.
    let (task, _review) = fixture.held_message(OTHER_INSTANCE, "from the pane").await;
    task.abort();
}

#[tokio::test]
async fn m8_a_denied_message_is_not_sent_and_the_next_asks_again() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    fixture.answer(&review, DENY).await;
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalDenied);
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Denied {
            permission_id: review.permission_id.clone()
        })
    );
    let (task, again) = fixture.held_message(INSTANCE, "hello?").await;
    assert_ne!(again.permission_id, review.permission_id);
    task.abort();
}

#[tokio::test]
async fn m8_a_message_waiting_on_its_review_is_withdrawn_by_the_mounts_release() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(
        last.phase,
        McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id,
            cause: McpAppWithdrawal::AppTornDown,
        }
    );
    assert_eq!(last.initiator, person_by("release"));
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m10_a_release_after_the_person_allowed_it_and_before_it_is_sent_stops_it() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // The allowing is slow to record; the mount is released meanwhile.
    *fixture.audit.slow.lock().unwrap() = Some(Duration::from_millis(300));
    fixture.answer(&review, ALLOW).await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(last.phase, McpAppAuditPhase::Refused(McpAppCode::Cancelled));
    assert_eq!(last.initiator, McpAppInitiator::System);
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m11_an_apps_message_waits_for_nobody_it_is_refused_while_a_turn_runs() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    // The person's turn runs, held.
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.person_sends("running", "think about it").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture
            .provider
            .executions
            .lock()
            .unwrap()
            .contains(&"running".to_owned())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.audit.records.lock().unwrap().clear();

    let result = fixture.send_message(INSTANCE, "me too").await;
    assert!(
        matches!(result, Err(ConversationError::TurnRunning)),
        "{result:?}"
    );
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::Refused(McpAppCode::TurnRunning),
        ]
    );
    // Nothing was queued behind the person's turn.
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.pending.is_empty());

    let _ = finish.send(());
    fixture.idle().await;
    // Once the turn is done it is taken.
    let execution = fixture.send_message(INSTANCE, "me too").await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), "me too");
}

#[tokio::test]
async fn m13_a_message_the_conversation_refuses_is_on_record_as_not_sent() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    fixture.audit.records.lock().unwrap().clear();
    let executions = fixture.provider.executions.lock().unwrap().len();
    // The conversation cannot take a message now.
    fixture
        .repository
        .verification_unreadable
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let result = fixture.send_message(INSTANCE, "hello").await;
    // Its own code, not one of an app's.
    assert!(
        matches!(result, Err(ConversationError::Metadata)),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    assert_eq!(phases[0], McpAppAuditPhase::Admitted);
    assert!(
        matches!(&phases[1], McpAppAuditPhase::MessageNotSent { .. }),
        "{phases:?}"
    );
    assert_eq!(phases.len(), 2);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m14_a_message_the_agent_took_without_its_evidence_is_on_record_as_sent() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    fixture
        .update_context(INSTANCE, Some("taken"), None)
        .await
        .unwrap();
    fixture.audit.records.lock().unwrap().clear();
    fixture
        .execution_audit
        .failing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let result = fixture.send_message(INSTANCE, "hello").await;
    assert!(
        matches!(result, Err(ConversationError::AdmissionEvidence { .. })),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    assert_eq!(phases[0], McpAppAuditPhase::Admitted);
    assert!(
        matches!(&phases[1], McpAppAuditPhase::MessageSent { .. }),
        "{phases:?}"
    );
    // Without its evidence the agent never ran it: what it carried was not
    // seen by the model, and goes with the next turn that runs.
    let app_turn = match &phases[1] {
        McpAppAuditPhase::MessageSent { execution_id } => execution_id.clone(),
        _ => unreachable!(),
    };
    fixture
        .execution_audit
        .failing
        .store(false, std::sync::atomic::Ordering::SeqCst);
    fixture.person_sends("next", "and now?").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("taken".to_owned())]
    );
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&app_turn));
}

#[tokio::test]
async fn m15a_a_message_whose_step_cannot_be_recorded_is_not_sent() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    fixture
        .audit
        .failing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        fixture.send_message(INSTANCE, "hello").await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    // The first message's review, too, is never shown unrecorded.
    assert!(matches!(
        fixture.send_message(OTHER_INSTANCE, "hello").await,
        Err(ConversationError::Audit)
    ));
    assert!(fixture.app_reviews().await.is_empty());
}

#[tokio::test]
async fn m16_allowing_a_mount_ends_with_its_release_and_with_the_opening() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    // The opening ends: the conversation is closed and opened again.
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    let (task, _review) = fixture.held_message(INSTANCE, "after the close").await;
    task.abort();

    let fixture = Fixture::new().await;
    fixture.allowed(OTHER_INSTANCE).await;
    fixture
        .service
        .release_app(
            fixture.id.clone(),
            caller("release"),
            fixture.app(OTHER_INSTANCE),
        )
        .await
        .unwrap();
    assert_eq!(
        refused(fixture.send_message(OTHER_INSTANCE, "still me").await),
        McpAppError::Cancelled
    );
}

#[tokio::test]
async fn an_apps_message_keeps_its_author_across_a_close_and_a_reopening() {
    let fixture = Fixture::new().await;
    let execution = fixture.allowed(INSTANCE).await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    // Read back from the conversation's saved history, not the live agent.
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == execution)
        .unwrap();
    assert_eq!(message.app, Some(written_by(&fixture)));
}

// --- mcp.updateModelContext -------------------------------------------------

#[tokio::test]
async fn c2_a_context_from_no_app_another_server_or_a_released_mount_is_refused() {
    let fixture = Fixture::new().await;
    let result = fixture
        .service
        .update_app_model_context(
            fixture.id.clone(),
            caller("app-context"),
            McpAppContextUpdate {
                app: fixture.app(INSTANCE),
                server: "files".into(),
                text: Some("x".into()),
                structured_content_json: None,
            },
        )
        .await;
    assert_eq!(refused(result), McpAppError::ServerMismatch);
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(
        refused(fixture.update_context(INSTANCE, Some("x"), None).await),
        McpAppError::Cancelled
    );
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Refused(McpAppCode::ServerMismatch),
            McpAppAuditPhase::Refused(McpAppCode::Cancelled),
        ]
    );
}

#[tokio::test]
async fn c3_c4_a_context_past_its_bound_or_with_structure_that_is_no_object_is_refused() {
    let fixture = Fixture::new().await;
    // The bound is on text and structure together, in UTF-8 bytes.
    let structured = r#"{"a":1}"#;
    let text = "é".repeat((AppModelContext::MAX_BYTES - structured.len()) / 2);
    let at_bound = format!(
        "{text}{}",
        "x".repeat(AppModelContext::MAX_BYTES - structured.len() - text.len())
    );
    assert_eq!(
        at_bound.len() + structured.len(),
        AppModelContext::MAX_BYTES
    );
    fixture
        .update_context(INSTANCE, Some(&at_bound), Some(structured))
        .await
        .unwrap();
    assert_eq!(
        refused(
            fixture
                .update_context(INSTANCE, Some(&format!("{at_bound}x")), Some(structured))
                .await
        ),
        McpAppError::RequestTooLarge
    );
    for not_an_object in ["[1]", "\"x\"", "{", "null"] {
        assert!(
            matches!(
                fixture
                    .update_context(INSTANCE, None, Some(not_an_object))
                    .await,
                Err(ConversationError::InvalidInput)
            ),
            "{not_an_object}"
        );
    }
    let phases = fixture.audit.phases();
    assert_eq!(
        phases[0],
        McpAppAuditPhase::ContextHeld {
            bytes: AppModelContext::MAX_BYTES,
            sequence: 1,
        }
    );
    assert_eq!(
        phases[1],
        McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge)
    );
    assert!(phases[2..]
        .iter()
        .all(|phase| phase == &McpAppAuditPhase::Refused(McpAppCode::InvalidRequest)));
    // What a refusal could not replace is still what is held.
    fixture.person_turn("next").await;
    assert_eq!(fixture.contexts_given("next").await, [Some(at_bound)]);
}

#[tokio::test]
async fn c5_c8_the_latest_context_goes_with_the_next_message_and_not_again_once_answered() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("April"), None)
        .await
        .unwrap();
    // Replaced: only the latest is held.
    fixture
        .update_context(INSTANCE, Some("May"), Some(r#"{"month": 5, "month": 6}"#))
        .await
        .unwrap();
    fixture.person_turn("next").await;
    let given = fixture.given("next").await;
    let contexts = given.app_model_context();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].text(), Some("May"));
    // Held as given: the context itself judges it, and nothing re-encodes it.
    assert_eq!(
        contexts[0].structured_content(),
        Some(r#"{"month": 5, "month": 6}"#)
    );
    assert_eq!(contexts[0].app().tool_id().as_str(), fixture.tool_id);
    assert_eq!(
        (
            contexts[0].app().tool().server(),
            contexts[0].app().tool().tool()
        ),
        (SERVER, UI_TOOL)
    );
    // Not shown as anything the person said.
    assert_eq!(given.text_str(), "and now?");
    assert_eq!(given.sender(), &MessageSender::Person);
    // Sent once: the message after carries none.
    fixture.person_turn("after").await;
    assert!(fixture.given("after").await.app_model_context().is_empty());
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::ContextHeld {
                bytes: 5,
                sequence: 1
            },
            McpAppAuditPhase::ContextHeld {
                bytes: 3 + r#"{"month": 5, "month": 6}"#.len(),
                sequence: 2,
            },
        ]
    );
}

#[tokio::test]
async fn c8_an_apps_own_message_carries_the_context_too() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    fixture
        .update_context(INSTANCE, Some("selected: row 3"), None)
        .await
        .unwrap();
    let execution = fixture
        .send_message(INSTANCE, "explain this row")
        .await
        .unwrap();
    assert_eq!(
        fixture.contexts_given(&execution).await,
        [Some("selected: row 3".to_owned())]
    );
}

#[tokio::test]
async fn c6_at_most_four_mounts_hold_a_context_and_a_message_frees_their_places() {
    let fixture = Fixture::new().await;
    let mounts = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
        "00000000-0000-4000-8000-000000000004",
    ];
    assert_eq!(mounts.len(), MAX_HELD_CONTEXTS);
    for mount in mounts {
        fixture
            .update_context(mount, Some(mount), None)
            .await
            .unwrap();
    }
    // A mount that holds one may replace it; a fifth may not hold one.
    fixture
        .update_context(mounts[0], Some("replaced"), None)
        .await
        .unwrap();
    assert!(matches!(
        fixture.update_context(INSTANCE, Some("fifth"), None).await,
        Err(ConversationError::Unavailable)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused(
            McpAppCode::TemporarilyUnavailable
        ))
    );
    // One message carries all four, in the order they were last given, and
    // frees their places.
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [
            Some(mounts[1].to_owned()),
            Some(mounts[2].to_owned()),
            Some(mounts[3].to_owned()),
            Some("replaced".to_owned()),
        ]
    );
    fixture
        .update_context(INSTANCE, Some("fifth"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn c7_an_empty_update_clears_what_the_mount_held() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    fixture
        .update_context(INSTANCE, Some(""), None)
        .await
        .unwrap();
    fixture.person_turn("next").await;
    assert!(fixture.given("next").await.app_model_context().is_empty());
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::ContextHeld {
                bytes: 1,
                sequence: 1
            },
            McpAppAuditPhase::ContextCleared { sequence: 2 },
        ]
    );
}

#[tokio::test]
async fn c9_a_message_that_is_refused_takes_no_context() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    // The person's turn runs, so the app's message is refused.
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture
        .update_context(OTHER_INSTANCE, Some("other"), None)
        .await
        .unwrap();
    fixture.person_sends("running", "think").await;
    // That turn took both.
    assert_eq!(
        fixture.contexts_given("running").await,
        [Some("kept".to_owned()), Some("other".to_owned())]
    );
    fixture
        .update_context(INSTANCE, Some("newer"), None)
        .await
        .unwrap();
    assert!(matches!(
        fixture.send_message(INSTANCE, "me too").await,
        Err(ConversationError::TurnRunning)
    ));
    let _ = finish.send(());
    fixture.idle().await;
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("newer".to_owned())]
    );
}

#[tokio::test]
async fn c10_a_retry_of_a_message_the_agent_has_carries_what_it_first_took() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("first"), None)
        .await
        .unwrap();
    fixture.person_turn("once").await;
    fixture
        .update_context(INSTANCE, Some("second"), None)
        .await
        .unwrap();
    // The same submission again: the agent's to settle, and no conflict.
    fixture.person_sends("once", "and now?").await;
    assert_eq!(
        fixture.contexts_given("once").await,
        [Some("first".to_owned())]
    );
    // What was held since is still held, for the next new message.
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("second".to_owned())]
    );
}

#[tokio::test]
async fn c11_c12_a_context_is_let_go_of_unsent_with_its_mount_or_its_opening() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("mine"), None)
        .await
        .unwrap();
    fixture
        .update_context(OTHER_INSTANCE, Some("other"), None)
        .await
        .unwrap();
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("other".to_owned())]
    );

    fixture
        .update_context(OTHER_INSTANCE, Some("again"), None)
        .await
        .unwrap();
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    fixture.person_turn("reopened").await;
    assert!(fixture
        .given("reopened")
        .await
        .app_model_context()
        .is_empty());
}

#[tokio::test]
async fn c14_a_context_whose_update_cannot_be_recorded_is_never_given() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("on record"), None)
        .await
        .unwrap();
    fixture
        .audit
        .failing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        fixture
            .update_context(INSTANCE, Some("unrecorded"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    assert!(matches!(
        fixture.update_context(INSTANCE, None, None).await,
        Err(ConversationError::Audit)
    ));
    fixture
        .audit
        .failing
        .store(false, std::sync::atomic::Ordering::SeqCst);
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("on record".to_owned())]
    );
}

/// Waits until `phase` is on record, past the first `from` records.
async fn on_record(fixture: &Fixture, from: usize, phase: McpAppAuditPhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.audit.phases()[from..].contains(&phase) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// The app's message, from the mount `instance`, as the request `request`, sent in
/// the background.
fn sending(
    fixture: &Fixture,
    instance: &str,
    request: &str,
    text: &str,
) -> tokio::task::JoinHandle<Result<String, ConversationError>> {
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let caller = fixture.caller(request);
    let message = McpAppMessage {
        app: fixture.app(instance),
        server: SERVER.into(),
        text: text.into(),
    };
    tokio::spawn(async move { service.send_app_message(id, caller, message).await })
}

#[tokio::test]
async fn m10_a_close_that_took_the_lock_first_refuses_an_admitted_message_and_opens_nothing() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    // Something holds the conversation's submission lock; the person's close
    // waits on it, and then the app's message, already admitted.
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let closing = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        tokio::spawn(async move { service.close(id, caller("close")).await })
    };
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let from = fixture.audit.phases().len();
    let message = sending(&fixture, INSTANCE, "late", "across a close");
    on_record(&fixture, from, McpAppAuditPhase::Admitted).await;
    let opened = fixture
        .provider
        .open_calls
        .load(std::sync::atomic::Ordering::SeqCst);
    drop(held);
    closing.await.unwrap().unwrap();
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(last.phase, McpAppAuditPhase::Refused(McpAppCode::Cancelled));
    assert_eq!(last.initiator, McpAppInitiator::System);
    // Not sent, and the closed conversation not opened again to refuse it.
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    assert_eq!(
        fixture
            .provider
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        opened
    );
}

#[tokio::test]
async fn m10_a_release_while_the_message_waits_for_the_lock_stops_it() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    let message = sending(&fixture, INSTANCE, "late", "after its release");
    on_record(&fixture, from, McpAppAuditPhase::Admitted).await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    drop(held);
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m10_a_release_after_the_lock_was_taken_and_before_the_enqueue_stops_it() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    // The message holds the lock, past its first check and its resolve,
    // held where the conversation's own record is read.
    let began = Arc::new(tokio::sync::Notify::new());
    let (open, gate) = oneshot::channel();
    *fixture.repository.verification_gate.lock().unwrap() = Some((began.clone(), gate));
    let message = sending(&fixture, INSTANCE, "late", "after its release");
    began.notified().await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    let _ = open.send(());
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m4b_a_first_message_whose_review_does_not_fit_is_refused_and_taken_once_allowed() {
    let fixture = Fixture::new().await;
    // Well within the input bound, but every quote is escaped twice in the
    // review that shows it.
    let quotes = "\"".repeat(4100);
    assert_eq!(
        refused(fixture.send_message(INSTANCE, &quotes).await),
        McpAppError::RequestTooLarge
    );
    assert!(fixture.app_reviews().await.is_empty());
    fixture.allowed(INSTANCE).await;
    let execution = fixture.send_message(INSTANCE, &quotes).await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), quotes);
}

#[tokio::test]
async fn m7b_allowing_one_first_message_sends_the_mounts_others_waiting() {
    let fixture = Fixture::new().await;
    let (one, review) = fixture.held_message(INSTANCE, "one").await;
    let (two, _) = fixture.held_message(INSTANCE, "two").await;
    assert_eq!(fixture.app_reviews().await.len(), 2);
    fixture.answer(&review, ALLOW).await;
    let first = one.await.unwrap();
    let second = two.await.unwrap();
    // One is sent; the other finds a turn running, or is sent after it —
    // either way, neither asks the person again.
    assert!(first.is_ok(), "{first:?}");
    assert!(
        matches!(second, Ok(_) | Err(ConversationError::TurnRunning)),
        "{second:?}"
    );
    assert!(fixture.app_reviews().await.is_empty());
    // Both by the person's one answer; the one it did not answer says which
    // review's answer allowed it.
    let mut approved = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match &record.phase {
            McpAppAuditPhase::Approved { with, .. } => {
                Some((with.clone(), record.initiator.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    approved.sort_by_key(|(with, _)| with.is_some());
    assert_eq!(
        approved,
        [
            (None, person_by("answer")),
            (Some(review.permission_id.clone()), person_by("answer")),
        ]
    );
}

#[tokio::test]
async fn m15b_a_message_whose_sending_cannot_be_recorded_is_the_agents_and_its_turn_withheld() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let taken = fixture.audit.records.lock().unwrap().len();
    // `Admitted` is written; `MessageSent` is not.
    *fixture.audit.failing_after.lock().unwrap() = Some(taken + 1);
    assert!(matches!(
        fixture.send_message(INSTANCE, "hello").await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Admitted)
    );
    // The agent has it all the same: the step was taken, and is said to be
    // unrecorded.
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.provider.executions.lock().unwrap().len() == executions {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn m17_the_same_request_again_is_the_same_turn() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let first = sending(&fixture, INSTANCE, "request-1", "hello")
        .await
        .unwrap()
        .unwrap();
    fixture.idle().await;
    let again = sending(&fixture, INSTANCE, "request-1", "hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, first);
    let turns = fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .iter()
        .filter(|id| **id == first)
        .count();
    assert_eq!(turns, 1);
    // Another text under the same request is another message: the agent's
    // to refuse as a conflict.
    let conflicting = sending(&fixture, INSTANCE, "request-1", "goodbye")
        .await
        .unwrap();
    assert!(
        matches!(
            conflicting,
            Err(ConversationError::Agent(AgentError::SubmissionConflict))
        ),
        "{conflicting:?}"
    );
    // And another mount's request of the same name is its own turn.
    fixture.allowed(OTHER_INSTANCE).await;
    let other = sending(&fixture, OTHER_INSTANCE, "request-1", "hello")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(other, first);
}

#[tokio::test]
async fn a_person_resending_an_apps_turn_as_theirs_is_a_conflict() {
    let fixture = Fixture::new().await;
    let execution = fixture.allowed(INSTANCE).await;
    let resent = fixture
        .service
        .submit(
            fixture.id.clone(),
            fixture.caller(&execution),
            execution.clone(),
            SubmittedMessage {
                text: "first".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(
            resent,
            Err(ConversationError::Agent(AgentError::SubmissionConflict))
        ),
        "{resent:?}"
    );
}

#[tokio::test]
async fn c9_a_message_refused_for_its_images_takes_none() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    // This gateway takes no images: refused before it carried anything.
    let refused = fixture
        .service
        .submit(
            fixture.id.clone(),
            fixture.caller("with-image"),
            "with-image".into(),
            SubmittedMessage {
                text: "look".into(),
                images: vec![SubmittedImage {
                    digest: format!("sha256:{}", "0".repeat(64)),
                    media_type: "image/png".into(),
                    size: 3,
                }],
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await;
    assert!(
        matches!(refused, Err(ConversationError::ImagesUnsupported)),
        "{refused:?}"
    );
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("kept".to_owned())]
    );
}

/// The update from the mount `instance` of `fixture`'s conversation, in the
/// background.
fn updating(
    fixture: &Fixture,
    instance: &str,
    text: Option<&str>,
) -> tokio::task::JoinHandle<Result<(), ConversationError>> {
    let service = fixture.service.clone();
    let id = fixture.id.clone();
    let update = McpAppContextUpdate {
        app: fixture.app(instance),
        server: SERVER.into(),
        text: text.map(str::to_owned),
        structured_content_json: None,
    };
    tokio::spawn(async move {
        service
            .update_app_model_context(id, caller("app-context"), update)
            .await
    })
}

#[tokio::test]
async fn c13_a_context_whose_update_is_still_being_recorded_goes_with_no_message() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("old"), None)
        .await
        .unwrap();
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    let pending = updating(&fixture, INSTANCE, Some("new"));
    hold.waiting.notified().await;
    // While "new" is being recorded the message takes what is held.
    fixture.person_turn("during").await;
    assert_eq!(
        fixture.contexts_given("during").await,
        [Some("old".to_owned())]
    );
    hold.go.add_permits(1);
    pending.await.unwrap().unwrap();
    fixture.person_turn("after").await;
    assert_eq!(
        fixture.contexts_given("after").await,
        [Some("new".to_owned())]
    );
}

#[tokio::test]
async fn c7b_a_mounts_updates_are_recorded_in_the_order_they_reach_it_and_the_later_stands() {
    for (first, second, carried) in [
        (Some("A"), None, Vec::new()),
        (None, Some("B"), vec![Some("B".to_owned())]),
    ] {
        let fixture = Fixture::new().await;
        let hold = Hold::default();
        *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
        let earlier = updating(&fixture, INSTANCE, first);
        hold.waiting.notified().await;
        // Given while the first is being recorded: it waits its turn.
        let later = updating(&fixture, INSTANCE, second);
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert!(fixture.audit.phases().is_empty());
        hold.go.add_permits(1);
        earlier.await.unwrap().unwrap();
        later.await.unwrap().unwrap();
        // On record in that order, numbered so.
        let sequences: Vec<u64> = fixture
            .audit
            .phases()
            .iter()
            .map(|phase| match phase {
                McpAppAuditPhase::ContextHeld { sequence, .. }
                | McpAppAuditPhase::ContextCleared { sequence } => *sequence,
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(sequences[0] < sequences[1], "{sequences:?}");
        assert_eq!(
            matches!(
                fixture.audit.phases()[0],
                McpAppAuditPhase::ContextHeld { .. }
            ),
            first.is_some()
        );
        fixture.person_turn("next").await;
        assert_eq!(fixture.contexts_given("next").await, carried);
    }
}

#[tokio::test]
async fn c8b_a_message_queued_behind_a_turn_carries_no_context_and_leaves_it_held() {
    let fixture = Fixture::new().await;
    // A turn runs, held.
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.person_sends("running", "think").await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    // Queued behind it: it carries none, so its removal or a reordering
    // can lose or invert nothing.
    fixture.person_sends("queued", "and then").await;
    let _ = finish.send(());
    fixture.idle().await;
    assert!(fixture.given("queued").await.app_model_context().is_empty());
    // The next message admitted with nothing running takes it.
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("kept".to_owned())]
    );
}

#[tokio::test]
async fn m10c_an_agent_stopped_without_the_lock_refuses_the_message_and_opens_nothing() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    // The message, admitted, waits for the submission lock; then holds it
    // past its first check, before its resolve, where pending mode changes
    // are read.
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    let message = sending(&fixture, INSTANCE, "late", "after the stop");
    on_record(&fixture, from, McpAppAuditPhase::Admitted).await;
    let began = Arc::new(tokio::sync::Notify::new());
    let (open, gate) = oneshot::channel();
    *fixture.repository.pending_gate.lock().unwrap() = Some((began.clone(), gate));
    drop(held);
    began.notified().await;
    // The desktop stops every agent: no lock taken.
    fixture.service.stop_active_agents().await.unwrap();
    let opened = fixture
        .provider
        .open_calls
        .load(std::sync::atomic::Ordering::SeqCst);
    let _ = open.send(());
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    assert_eq!(
        fixture
            .provider
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        opened
    );
}

#[tokio::test]
async fn m17b_a_retry_of_a_sent_message_after_a_reopening_is_not_asked_again() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let first = sending(&fixture, INSTANCE, "request-1", "hello")
        .await
        .unwrap()
        .unwrap();
    fixture.idle().await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    // Opened again, the mount is not allowed in this opening; the retry is
    // a turn the agent has, so nobody is asked.
    let again = sending(&fixture, INSTANCE, "request-1", "hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, first);
    assert!(fixture.app_reviews().await.is_empty());
}

#[tokio::test]
async fn c8_a_context_names_the_update_that_gave_it_as_its_record_does() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    let call = fixture.audit.records.lock().unwrap()[0].call_id.clone();
    fixture.person_turn("next").await;
    let given = fixture.given("next").await;
    assert_eq!(given.app_model_context()[0].update_id(), call);
}

#[tokio::test]
async fn m8_a_first_message_nobody_answers_expires_and_is_not_sent() {
    let fixture = Fixture::new().await;
    let (task, _review) = fixture.held_message(INSTANCE, "hello").await;
    tokio::time::pause();
    tokio::time::advance(APP_REVIEW_DEADLINE + Duration::from_secs(1)).await;
    tokio::time::resume();
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalExpired);
    assert!(matches!(
        fixture.audit.phases().last(),
        Some(McpAppAuditPhase::Expired { .. })
    ));
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

/// Close, then open again through an app's context update with the
/// provider's opening held: the opening is live, its agent still attaching,
/// and nothing runs or waits.
async fn reopened_while_attaching(fixture: &Fixture) -> oneshot::Sender<()> {
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    let (open, gate) = oneshot::channel();
    *fixture.provider.open_gate.lock().unwrap() = Some(gate);
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    open
}

#[tokio::test]
async fn c8d_a_context_carried_by_a_turn_removed_or_whose_agent_never_attached_is_kept() {
    // Removed before it ran.
    let fixture = Fixture::new().await;
    let open = reopened_while_attaching(&fixture).await;
    fixture.person_sends("p1", "first").await;
    assert!(fixture
        .service
        .remove(fixture.id.clone(), caller("remove"), "p1".into())
        .await
        .unwrap());
    let _ = open.send(());
    // Not lost: the next turn carries it, or — admitted while the agent
    // finished attaching, so not idle — leaves it held for the one after.
    fixture.person_sends("p2", "second").await;
    let carried = fixture.contexts_given("p2").await;
    let held = fixture.service.apps_of(&fixture.id).held_contexts().len();
    assert!(
        carried == [Some("ctx".to_owned())] || (carried.is_empty() && held == 1),
        "carried {carried:?}, held {held}"
    );
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&"p1".to_owned()));

    // Failed before it ran: its agent could not attach.
    let fixture = Fixture::new().await;
    let open = reopened_while_attaching(&fixture).await;
    *fixture.provider.open_failure.lock().unwrap() =
        Some(nessa_sdk::application::agent_execution::agents::AgentError::Protocol("no".into()));
    fixture.person_sends("p1", "first").await;
    let _ = open.send(());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = fixture
                .service
                .read(fixture.id.clone(), caller("read"))
                .await;
            let settled = view.is_ok_and(|view| {
                view.messages.iter().any(|message| {
                    message.execution_id == "p1"
                        && !matches!(
                            message.status,
                            ConversationMessageStatus::Queued | ConversationMessageStatus::Running
                        )
                })
            });
            if settled {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&"p1".to_owned()));
    assert_eq!(
        fixture.service.apps_of(&fixture.id).held_contexts().len(),
        1
    );
}

/// The contexts held in `fixture`'s conversation, none a turn carries.
fn held_count(fixture: &Fixture) -> usize {
    fixture.service.apps_of(&fixture.id).held_contexts().len()
}

/// Until `execution`'s message settled, and what it carried with it.
async fn settled(fixture: &Fixture, execution: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = fixture
                .service
                .read(fixture.id.clone(), caller("read"))
                .await;
            if view.is_ok_and(|view| {
                view.messages.iter().any(|message| {
                    message.execution_id == execution
                        && !matches!(
                            message.status,
                            ConversationMessageStatus::Queued | ConversationMessageStatus::Running
                        )
                })
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.service.apps_of(&fixture.id).carrying() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn c8d_a_context_carried_by_a_turn_whose_prompt_never_reached_the_agent_is_kept() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    // It ran — it was scheduled and selected — but failed as it was
    // prepared, before its prompt went: the agent never saw the context.
    *fixture.provider.prepare_failure.lock().unwrap() = Some(AgentError::Protocol("no".into()));
    fixture.person_sends("p1", "first").await;
    settled(&fixture, "p1").await;
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&"p1".to_owned()));
    assert_eq!(held_count(&fixture), 1);
}

#[tokio::test]
async fn c8d_a_context_carried_by_a_turn_the_adapter_failed_before_its_prompt_is_kept() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    // The adapter's own report, with no result from the provider: its
    // process gone between turns, say, before the prompt was built.
    *fixture.provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
        ExecutionReport::new(None, Some(AgentError::Closed), ProviderSessionState::Usable),
    ));
    *fixture
        .provider
        .execution_observation_failure
        .lock()
        .unwrap() = Some(ObservationFailure::new(
        AgentError::Closed,
        ObservationFailureCause::ExecutionFailed,
    ));
    fixture.person_sends("p1", "first").await;
    settled(&fixture, "p1").await;
    // A report, and nothing from the agent: no answer, and kept.
    assert_eq!(held_count(&fixture), 1);
}

#[tokio::test]
async fn c9_a_message_the_agent_refused_after_it_carried_the_contexts_leaves_them_held() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    // The agent refuses it as it is admitted: it cannot save it.
    fixture
        .storage_refuses
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = fixture
        .service
        .submit(
            fixture.id.clone(),
            fixture.caller("refused"),
            "refused".into(),
            SubmittedMessage {
                text: "hello".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await;
    fixture
        .storage_refuses
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(
        matches!(
            refused,
            Err(ConversationError::Agent(AgentError::Storage(_)))
        ),
        "{refused:?}"
    );
    // Nothing carries it: it is held, and goes with the next.
    assert!(!fixture.service.apps_of(&fixture.id).carrying());
    assert_eq!(held_count(&fixture), 1);
    fixture.person_sends("next", "again").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("ctx".to_owned())]
    );
}

#[tokio::test]
async fn c8c_a_context_carried_by_a_turn_the_agent_answered_with_a_failure_is_let_go_of() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    *fixture.provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Err(AgentError::Protocol("failed".into()))),
            None,
            ProviderSessionState::Usable,
        )));
    // Its stream ends with it, as an ACP worker's does after a failed turn.
    *fixture
        .provider
        .execution_observation_failure
        .lock()
        .unwrap() = Some(ObservationFailure::new(
        AgentError::Protocol("failed".into()),
        ObservationFailureCause::ExecutionFailed,
    ));
    fixture.person_sends("p1", "first").await;
    assert_eq!(fixture.contexts_given("p1").await, [Some("ctx".to_owned())]);
    settled(&fixture, "p1").await;
    // The agent answered for the turn that held it: it saw it, and it goes
    // no more.
    assert_eq!(held_count(&fixture), 0);
}

#[tokio::test]
async fn c8b_a_message_steered_into_a_turn_carries_no_context() {
    let fixture = Fixture::new().await;
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.person_sends("running", "think").await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    fixture
        .service
        .submit(
            fixture.id.clone(),
            fixture.caller("steered"),
            "steered".into(),
            SubmittedMessage {
                text: "and also".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Steer,
        )
        .await
        .unwrap();
    let _ = finish.send(());
    fixture.idle().await;
    assert!(fixture
        .given("steered")
        .await
        .app_model_context()
        .is_empty());
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("kept".to_owned())]
    );
}

#[tokio::test]
async fn c7b_one_mounts_update_being_recorded_holds_up_no_other_mounts() {
    let fixture = Fixture::new().await;
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    let first = updating(&fixture, INSTANCE, Some("A"));
    hold.waiting.notified().await;
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.update_context(OTHER_INSTANCE, Some("B"), None),
    )
    .await
    .expect("another mount's update waited on this one's record")
    .unwrap();
    hold.go.add_permits(1);
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn c15_a_release_after_an_updates_number_and_before_its_hold_leaves_nothing_held() {
    let fixture = Fixture::new().await;
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    let update = updating(&fixture, INSTANCE, Some("late"));
    hold.waiting.notified().await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    hold.go.add_permits(1);
    // On record, and answered applied: the release came after it.
    update.await.unwrap().unwrap();
    assert!(matches!(
        fixture.audit.phases().last(),
        Some(McpAppAuditPhase::ContextHeld { .. })
    ));
    fixture.person_turn("next").await;
    assert!(fixture.given("next").await.app_model_context().is_empty());
}

#[tokio::test]
async fn m1_c1_messages_and_contexts_take_one_of_the_gateways_app_call_slots() {
    let fixture = Fixture::new().await;
    let all = u32::try_from(crate::conversation::application::MAX_APP_CALLS).unwrap();
    let taken = fixture
        .service
        .inner
        .app_calls
        .clone()
        .acquire_many_owned(all)
        .await
        .unwrap();
    assert!(matches!(
        fixture.send_message(INSTANCE, "hello").await,
        Err(ConversationError::Unavailable)
    ));
    assert!(matches!(
        fixture.update_context(INSTANCE, Some("x"), None).await,
        Err(ConversationError::Unavailable)
    ));
    // Nothing recorded, nothing asked.
    assert!(fixture.audit.phases().is_empty());
    drop(taken);
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn m8_a_first_message_whose_caller_went_is_withdrawn_on_record() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture
            .audit
            .phases()
            .contains(&McpAppAuditPhase::Withdrawn {
                permission_id: review.permission_id.clone(),
                cause: McpAppWithdrawal::RequestCancelled,
            })
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(fixture.app_reviews().await.is_empty());
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m10_a_message_admitted_in_one_opening_is_not_sent_into_another() {
    let fixture = Fixture::new().await;
    fixture.allowed(INSTANCE).await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    let message = sending(&fixture, INSTANCE, "late", "into the old opening");
    on_record(&fixture, from, McpAppAuditPhase::Admitted).await;
    // Its opening ends without the lock, and another begins.
    fixture.service.stop_active_agents().await.unwrap();
    fixture
        .update_context(OTHER_INSTANCE, Some("opens it again"), None)
        .await
        .unwrap();
    drop(held);
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[test]
fn a_message_carries_as_many_contexts_as_a_conversation_holds() {
    // One owner: the conversation holds no more than one message carries.
    assert_eq!(MAX_HELD_CONTEXTS, UserMessage::MAX_APP_MODEL_CONTEXTS);
}
