//! An MCP App in its conversation (#390): `mcp.sendMessage` and
//! `mcp.updateModelContext` through the conversation service, at least one
//! test per row of the tables in `docs/design/mcp-app-calls.md` ("An app in
//! its conversation: the gateway"), named by row — a message asked about
//! every time, sent as the person's turn and attributed to the app, refused
//! while a turn runs; a context held per mount, carried by the next message
//! admitted while the conversation is idle and let go of once that message
//! is saved, replaced, cleared, bounded, and dropped with its mount or its
//! opening — and the audit each leaves.
use super::*;
use crate::app_call_test_support::{
    caller, Fixture, Hold, INSTANCE, OTHER_INSTANCE, SERVER, UI_TOOL,
};
use crate::conversation::application::app_reviews::{
    ALLOW, APP_REVIEW_DEADLINE, DENY, MAX_HELD_CONTEXTS, MAX_OPEN_APP_REVIEWS,
};
use crate::conversation::application::view::{
    ConversationMessageApp, ConversationMessageStatus, ConversationPermissionOrigin,
};
use crate::conversation::application::{ContextDrop, McpAppAsk};
use crate::conversation::application::{
    ConversationLimits, SubmissionMode, SubmittedImage, SubmittedMessage, MAX_APP_CALLS,
};
use crate::product_contract::generated::ConversationErrorCode;
use nessa_auth::domain::PrincipalId;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::domain::agent_execution::prompts::{AppModelContext, MessageSender, UserMessage};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Notify};

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
    /// Wait until every turn has finished and nothing waits.
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
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    /// The app's message from the mount `instance`, asked about, allowed by
    /// the person, and answered.
    async fn sent(&self, instance: &str, text: &str) -> Result<String, ConversationError> {
        let (task, review) = self.held_message(instance, text).await;
        self.answer(&review, ALLOW).await;
        task.await.unwrap()
    }

    /// [`Self::sent`], as the app's request `request`.
    async fn sent_as(
        &self,
        instance: &str,
        request: &str,
        text: &str,
    ) -> Result<String, ConversationError> {
        let (task, review) = self.asked(instance, request, text).await;
        self.answer(&review, ALLOW).await;
        task.await.unwrap()
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

    /// The contexts the conversation's apps hold now, as text.
    fn held_now(&self) -> Vec<String> {
        self.service
            .apps_of(&self.id)
            .held()
            .iter()
            .map(|context| context.text().unwrap_or_default().to_owned())
            .collect()
    }

    /// The agent's admission of the next submission held where it records
    /// its evidence — past the conversation's last check, before the agent
    /// answered — until the returned sender is used.
    fn hold_admission(&self) -> (Arc<Notify>, oneshot::Sender<()>) {
        let began = Arc::new(Notify::new());
        let (go, gate) = oneshot::channel();
        *self.execution_audit.hold.lock().unwrap() = Some((began.clone(), gate));
        (began, go)
    }
}

/// Waits until `phase` is on record, past the first `from` records.
async fn on_record(fixture: &Fixture, from: usize, phase: &McpAppAuditPhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.audit.phases()[from..].contains(phase) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// Waits until an approval is on record, past the first `from` records.
async fn approved_on_record(fixture: &Fixture, from: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.audit.phases()[from..]
            .iter()
            .any(|phase| matches!(phase, McpAppAuditPhase::Approved { .. }))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// The drops on record, each with who dropped it and why — each recorded
/// against the update that held what it dropped, whose `ContextHeld` is on
/// record before it.
fn drops(fixture: &Fixture) -> Vec<(ContextDrop, McpAppInitiator)> {
    let records = fixture.audit.records.lock().unwrap().clone();
    records
        .iter()
        .enumerate()
        .filter_map(|(at, record)| match record.phase {
            McpAppAuditPhase::ContextDropped { cause } => {
                assert!(
                    records[..at].iter().any(|held| {
                        matches!(held.phase, McpAppAuditPhase::ContextHeld { .. })
                            && held.call_id == record.call_id
                            && held.request_id == record.request_id
                            && held.app == record.app
                            && held.ask == record.ask
                    }),
                    "a drop of no update on record: {record:?}"
                );
                Some((cause, record.initiator.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The app's message, from the mount `instance`, as the request `request`,
/// sent in the background.
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

/// Until `execution`'s message settled.
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
}

/// The person's turn `execution` started, and held running until the
/// returned sender is used.
async fn running(fixture: &Fixture, execution: &str) -> oneshot::Sender<()> {
    let (finish, gate) = oneshot::channel();
    *fixture.provider.execution_gate.lock().unwrap() = Some(gate);
    fixture.person_sends(execution, "think about it").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture
            .provider
            .executions
            .lock()
            .unwrap()
            .contains(&execution.to_owned())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    finish
}

// --- mcp.sendMessage ------------------------------------------------------

#[tokio::test]
async fn m1_c1_messages_and_contexts_take_one_of_the_gateways_app_call_slots() {
    let fixture = Fixture::new().await;
    let all = u32::try_from(MAX_APP_CALLS).unwrap();
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
    // Nothing recorded, nothing asked, nothing held.
    assert!(fixture.audit.phases().is_empty());
    assert!(fixture.app_reviews().await.is_empty());
    assert!(fixture.held_now().is_empty());
    drop(taken);
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    assert_eq!(fixture.held_now(), ["x"]);
}

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
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
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
    assert!(fixture.app_reviews().await.is_empty());
    // Exactly at the bound is a message: it asks to be sent, and is.
    let execution = fixture.sent(INSTANCE, &"x".repeat(bound)).await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str().len(), bound);
}

#[tokio::test]
async fn m5_a_released_mount_sends_nothing_and_asks_nobody() {
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
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m6_m17_the_same_request_again_is_the_same_turn_and_nobody_is_asked_again() {
    let fixture = Fixture::new().await;
    let first = fixture
        .sent_as(INSTANCE, "request-1", "hello")
        .await
        .unwrap();
    fixture.idle().await;
    let from = fixture.audit.phases().len();
    // A retry of a turn the agent has: no review, the same turn.
    let again = tokio::time::timeout(
        Duration::from_secs(5),
        sending(&fixture, INSTANCE, "request-1", "hello"),
    )
    .await
    .expect("a retry of a turn the agent has asked the person again")
    .unwrap()
    .unwrap();
    assert_eq!(again, first);
    assert_eq!(
        fixture.audit.phases()[from..],
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::MessageSent {
                execution_id: first.clone(),
                code: None,
            },
        ]
    );
    assert!(fixture.app_reviews().await.is_empty());
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
    // to refuse as a conflict, and on record as not sent.
    let from = fixture.audit.phases().len();
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
    assert_eq!(
        fixture.audit.phases()[from..],
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::MessageNotSent {
                execution_id: first.clone(),
                code: ConversationErrorCode::SubmissionConflict,
            },
        ]
    );
    // Another mount's request of the same name is its own turn, and asks.
    let other = fixture
        .sent_as(OTHER_INSTANCE, "request-1", "hello")
        .await
        .unwrap();
    assert_ne!(other, first);
}

#[tokio::test]
async fn m17_a_retry_of_a_sent_message_after_a_reopening_is_not_asked_again() {
    let fixture = Fixture::new().await;
    let first = fixture
        .sent_as(INSTANCE, "request-1", "hello")
        .await
        .unwrap();
    fixture.idle().await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    // Opened again: the retry is a turn the restored agent has.
    let again = tokio::time::timeout(
        Duration::from_secs(5),
        sending(&fixture, INSTANCE, "request-1", "hello"),
    )
    .await
    .expect("a retry after a reopening asked the person again")
    .unwrap()
    .unwrap();
    assert_eq!(again, first);
    assert!(fixture.app_reviews().await.is_empty());
}

#[tokio::test]
async fn m6b_the_same_request_while_it_is_in_flight_is_refused() {
    let fixture = Fixture::new().await;
    let (first, review) = fixture.asked(INSTANCE, "request-1", "hello").await;
    // The same request again while the first is in review: one review, and
    // the second refused by the system, on record.
    let from = fixture.audit.phases().len();
    let second = sending(&fixture, INSTANCE, "request-1", "hello")
        .await
        .unwrap();
    assert!(
        matches!(second, Err(ConversationError::Unavailable)),
        "{second:?}"
    );
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records.len(), from + 1);
    assert_eq!(
        records[from].phase,
        McpAppAuditPhase::Refused(McpAppCode::TemporarilyUnavailable)
    );
    assert_eq!(records[from].initiator, McpAppInitiator::System);
    assert_eq!(fixture.app_reviews().await.len(), 1);
    // The first goes on, and once it settled the same request is a retry of
    // its turn (row M6): not asked again.
    fixture.answer(&review, ALLOW).await;
    let sent = first.await.unwrap().unwrap();
    fixture.idle().await;
    let again = tokio::time::timeout(
        Duration::from_secs(5),
        sending(&fixture, INSTANCE, "request-1", "hello"),
    )
    .await
    .expect("a retry once the first settled asked the person again")
    .unwrap()
    .unwrap();
    assert_eq!(again, sent);
}

#[tokio::test]
async fn m6b_a_request_whose_caller_went_is_free_to_be_sent_again() {
    let fixture = Fixture::new().await;
    let (first, review) = fixture.asked(INSTANCE, "request-1", "hello").await;
    first.abort();
    let _ = first.await;
    on_record(
        &fixture,
        0,
        &McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id.clone(),
            cause: McpAppWithdrawal::RequestCancelled,
        },
    )
    .await;
    // The call ended when its caller went — its task is done — and the same
    // request asks again.
    assert!(
        fixture
            .service
            .app_calls_finished(Duration::from_secs(5))
            .await
    );
    let (again, review) = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.asked(INSTANCE, "request-1", "hello"),
    )
    .await
    .expect("the request was still in flight after its caller went");
    fixture.answer(&review, ALLOW).await;
    again.await.unwrap().unwrap();
}

#[tokio::test]
async fn m7_m8_m12_a_message_asks_and_allowed_lands_as_the_persons_turn_written_by_the_app() {
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
                },
                person_by("answer")
            ),
            (
                McpAppAuditPhase::MessageSent {
                    execution_id: execution.clone(),
                    code: None,
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
async fn m7_every_message_asks_and_allowing_one_allows_no_other() {
    let fixture = Fixture::new().await;
    fixture.sent(INSTANCE, "first").await.unwrap();
    fixture.idle().await;
    // The same mount's next message asks again.
    let (task, review) = fixture.held_message(INSTANCE, "second").await;
    assert_eq!(fixture.app_reviews().await.len(), 1);
    fixture.answer(&review, ALLOW).await;
    task.await.unwrap().unwrap();
    fixture.idle().await;
    // Two at once: allowing one leaves the other waiting on its own.
    let (one, first) = fixture.held_message(INSTANCE, "one").await;
    let (two, second) = fixture.held_message(INSTANCE, "two").await;
    fixture.answer(&first, ALLOW).await;
    one.await.unwrap().unwrap();
    let open = fixture.app_reviews().await;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].permission_id, second.permission_id);
    assert!(!two.is_finished());
    fixture.answer(&second, DENY).await;
    assert_eq!(refused(two.await.unwrap()), McpAppError::ApprovalDenied);
}

#[tokio::test]
async fn m7b_a_message_whose_review_does_not_fit_is_refused_and_no_review_is_opened() {
    let fixture = Fixture::new().await;
    // Well within the input bound, but every quote is escaped twice in the
    // review that shows it.
    let quotes = "\"".repeat(4100);
    for _ in 0..2 {
        assert_eq!(
            refused(fixture.send_message(INSTANCE, &quotes).await),
            McpAppError::RequestTooLarge
        );
    }
    assert!(fixture.app_reviews().await.is_empty());
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge),
            McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge),
        ]
    );
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m7c_with_the_reviews_full_a_message_is_withdrawn_and_unavailable() {
    let fixture = Fixture::new().await;
    let mut waiting = Vec::new();
    for n in 0..MAX_OPEN_APP_REVIEWS {
        waiting.push(fixture.held_message(INSTANCE, &format!("m{n}")).await.0);
    }
    let from = fixture.audit.phases().len();
    assert!(matches!(
        fixture.send_message(INSTANCE, "one too many").await,
        Err(ConversationError::Unavailable)
    ));
    let records = fixture.audit.records.lock().unwrap()[from..].to_vec();
    assert_eq!(records.len(), 2);
    let McpAppAuditPhase::ApprovalRequested { permission_id } = &records[0].phase else {
        panic!("{records:?}");
    };
    assert_eq!(
        records[1].phase,
        McpAppAuditPhase::Withdrawn {
            permission_id: permission_id.clone(),
            cause: McpAppWithdrawal::RequestCancelled,
        }
    );
    assert_eq!(records[1].initiator, McpAppInitiator::System);
    assert_eq!(fixture.app_reviews().await.len(), MAX_OPEN_APP_REVIEWS);
    for task in waiting {
        task.abort();
    }
}

#[tokio::test]
async fn m9_a_denied_message_is_not_sent_and_the_next_asks_again() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    fixture.answer(&review, DENY).await;
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalDenied);
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(
        last.phase,
        McpAppAuditPhase::Denied {
            permission_id: review.permission_id.clone()
        }
    );
    assert_eq!(last.initiator, person_by("answer"));
    let (task, again) = fixture.held_message(INSTANCE, "hello?").await;
    assert_ne!(again.permission_id, review.permission_id);
    task.abort();
}

#[tokio::test]
async fn m9_a_message_nobody_answers_expires_and_is_not_sent() {
    let fixture = Fixture::new().await;
    let (task, _review) = fixture.held_message(INSTANCE, "hello").await;
    tokio::time::pause();
    tokio::time::advance(APP_REVIEW_DEADLINE + Duration::from_secs(1)).await;
    tokio::time::resume();
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalExpired);
    let records = fixture.audit.records.lock().unwrap().clone();
    assert!(matches!(
        records.last().unwrap().phase,
        McpAppAuditPhase::Expired { .. }
    ));
    assert_eq!(records.last().unwrap().initiator, McpAppInitiator::System);
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m9_a_message_whose_caller_went_is_withdrawn_on_record() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    task.abort();
    let _ = task.await;
    on_record(
        &fixture,
        0,
        &McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id.clone(),
            cause: McpAppWithdrawal::RequestCancelled,
        },
    )
    .await;
    assert!(fixture.app_reviews().await.is_empty());
    assert_eq!(fixture.provider.executions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn m9_a_message_waiting_on_its_review_is_withdrawn_by_the_mounts_release() {
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
async fn m9_a_message_waiting_on_its_review_is_withdrawn_by_a_close() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(
        last.phase,
        McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id,
            cause: McpAppWithdrawal::ConversationEnded,
        }
    );
    assert_eq!(last.initiator, person_by("close"));
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
async fn m10_a_close_that_took_the_lock_first_refuses_an_allowed_message_and_opens_nothing() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "across a close").await;
    // Something holds the conversation's submission lock; the person's close
    // waits on it, and then the app's message, once allowed.
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
    fixture.answer(&review, ALLOW).await;
    approved_on_record(&fixture, from).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let opened = fixture.provider.open_calls.load(Ordering::SeqCst);
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
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), opened);
}

#[tokio::test]
async fn m10_a_release_while_the_message_waits_for_the_lock_stops_it() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "after its release").await;
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    fixture.answer(&review, ALLOW).await;
    approved_on_record(&fixture, from).await;
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
async fn m10_a_release_before_the_submission_lock_refuses_the_message() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "after its release").await;
    // The message holds the lock, past its first check and its resolve,
    // held where the conversation's own record is read.
    let began = Arc::new(Notify::new());
    let (open, gate) = oneshot::channel();
    *fixture.repository.verification_gate.lock().unwrap() = Some((began.clone(), gate));
    fixture.answer(&review, ALLOW).await;
    began.notified().await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    let _ = open.send(());
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(last.phase, McpAppAuditPhase::Refused(McpAppCode::Cancelled));
    assert_eq!(last.initiator, McpAppInitiator::System);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m10_an_agent_stopped_without_the_lock_refuses_the_message_and_opens_nothing() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "after the stop").await;
    // The message, allowed, waits for the submission lock; then holds it
    // past its first check, before its resolve, where pending mode changes
    // are read.
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    fixture.answer(&review, ALLOW).await;
    approved_on_record(&fixture, from).await;
    let began = Arc::new(Notify::new());
    let (open, gate) = oneshot::channel();
    *fixture.repository.pending_gate.lock().unwrap() = Some((began.clone(), gate));
    drop(held);
    began.notified().await;
    // The desktop stops every agent: no lock taken.
    fixture.service.stop_active_agents().await.unwrap();
    let opened = fixture.provider.open_calls.load(Ordering::SeqCst);
    let _ = open.send(());
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    assert_eq!(fixture.provider.open_calls.load(Ordering::SeqCst), opened);
}

#[tokio::test]
async fn m10_a_message_admitted_in_one_opening_is_not_sent_into_another() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "into the old opening").await;
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let from = fixture.audit.phases().len();
    fixture.answer(&review, ALLOW).await;
    approved_on_record(&fixture, from).await;
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

#[tokio::test]
async fn m10_a_gateway_stop_after_the_person_allowed_it_sends_nothing_and_is_not_unresolved() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "hello").await;
    // The person's allowing is held mid-record; the gateway stops meanwhile.
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    fixture.answer(&review, ALLOW).await;
    hold.waiting.notified().await;
    let stopping = {
        let service = fixture.service.clone();
        tokio::spawn(async move { service.shutdown().await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.service.inner.retirement.get().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    hold.go.add_permits(1);
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records.last().unwrap();
    assert_eq!(last.phase, McpAppAuditPhase::Refused(McpAppCode::Cancelled));
    assert_eq!(last.initiator, McpAppInitiator::System);
    assert!(!records
        .iter()
        .any(|record| matches!(record.phase, McpAppAuditPhase::MessageUnresolved { .. })));
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    let _ = stopping.await.unwrap();
}

#[tokio::test]
async fn m10_a_delete_that_took_the_lock_first_refuses_an_allowed_message() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (message, review) = fixture.held_message(INSTANCE, "across a delete").await;
    // Something holds the conversation's submission lock; the person's
    // delete waits on it, and then the app's message, once allowed.
    let held = fixture.service.inner.mode_changes.lock(&fixture.id).await;
    let deleting = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        tokio::spawn(async move { service.delete(id, caller("delete")).await })
    };
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    let from = fixture.audit.phases().len();
    fixture.answer(&review, ALLOW).await;
    approved_on_record(&fixture, from).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    drop(held);
    // The fixture names no eraser for the agent's own session: the delete
    // happens, and may say its erasure is unfinished.
    let deleted = deleting.await.unwrap();
    assert!(
        matches!(
            deleted,
            Ok(_) | Err(ConversationError::DeletionIncomplete(_))
        ),
        "{deleted:?}"
    );
    assert_eq!(refused(message.await.unwrap()), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let last = records
        .iter()
        .rev()
        .find(|record| {
            record.ask
                == McpAppAsk::SendMessage {
                    server: SERVER.into(),
                }
        })
        .unwrap();
    assert_eq!(last.phase, McpAppAuditPhase::Refused(McpAppCode::Cancelled));
    assert_eq!(last.initiator, McpAppInitiator::System);
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m11_an_apps_message_is_refused_while_the_persons_input_waits_and_nothing_runs() {
    let fixture = Fixture::new().await;
    // The person's message waits as its turn is prepared: the view shows it
    // waiting, and nothing running.
    let began = Arc::new(Notify::new());
    let (go, gate) = oneshot::channel();
    *fixture.provider.prepare_gate.lock().unwrap() = Some((began.clone(), gate));
    fixture.person_sends("waiting", "after you").await;
    began.notified().await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(
        !view
            .messages
            .iter()
            .any(|message| message.status == ConversationMessageStatus::Running),
        "{:?}",
        view.messages
    );
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&"waiting".to_owned()));
    let from = fixture.audit.phases().len();
    let result = fixture.sent(INSTANCE, "me too").await;
    assert!(
        matches!(result, Err(ConversationError::TurnRunning)),
        "{result:?}"
    );
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused(McpAppCode::TurnRunning))
    );
    assert!(fixture.audit.phases().len() > from);
    let _ = go.send(());
    fixture.idle().await;
    let execution = fixture.sent(INSTANCE, "me too").await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), "me too");
}

#[tokio::test]
async fn m11_an_apps_message_waits_for_nobody_it_is_refused_while_a_turn_runs() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    // The person's turn runs, held; it carried the context.
    let finish = running(&fixture, "running").await;
    assert_eq!(
        fixture.contexts_given("running").await,
        [Some("kept".to_owned())]
    );
    fixture
        .update_context(INSTANCE, Some("newer"), None)
        .await
        .unwrap();
    let from = fixture.audit.phases().len();
    let result = fixture.sent(INSTANCE, "me too").await;
    assert!(
        matches!(result, Err(ConversationError::TurnRunning)),
        "{result:?}"
    );
    let phases = fixture.audit.phases()[from..].to_vec();
    assert!(matches!(
        phases[..],
        [
            McpAppAuditPhase::ApprovalRequested { .. },
            McpAppAuditPhase::Approved { .. },
            McpAppAuditPhase::Refused(McpAppCode::TurnRunning),
        ]
    ));
    // Nothing was queued behind the person's turn, and nothing let go of.
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view.pending.is_empty());
    assert_eq!(fixture.held_now(), ["newer"]);

    let _ = finish.send(());
    fixture.idle().await;
    // Once the turn is done it is taken, with what is held.
    let execution = fixture.sent(INSTANCE, "me too").await.unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), "me too");
    assert_eq!(
        fixture.contexts_given(&execution).await,
        [Some("newer".to_owned())]
    );
}

#[tokio::test]
async fn m13_a_message_the_conversation_refuses_is_on_record_as_not_sent() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // The conversation cannot take a message now.
    fixture
        .repository
        .verification_unreadable
        .store(true, Ordering::SeqCst);
    fixture.answer(&review, ALLOW).await;
    let result = task.await.unwrap();
    // Its own code, not one of an app's.
    assert!(
        matches!(result, Err(ConversationError::Metadata)),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    assert!(
        matches!(
            phases.last().unwrap(),
            McpAppAuditPhase::MessageNotSent { .. }
        ),
        "{phases:?}"
    );
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m14_c13_a_message_taken_without_its_evidence_is_sent_and_what_it_carried_is_lost() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("taken"), None)
        .await
        .unwrap();
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    fixture
        .execution_audit
        .failing
        .store(true, Ordering::SeqCst);
    fixture.answer(&review, ALLOW).await;
    let result = task.await.unwrap();
    assert!(
        matches!(result, Err(ConversationError::AdmissionEvidence { .. })),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    // On record as sent, with what failed (row M14).
    let Some(McpAppAuditPhase::MessageSent {
        execution_id,
        code: Some(ConversationErrorCode::AuditUnavailable),
    }) = phases.last()
    else {
        panic!("{phases:?}");
    };
    let app_turn = execution_id.clone();
    // The agent saved it, so what it carried was let go of; without its
    // evidence the agent never ran it, so the model never saw it: lost,
    // and the app may give it again (recorded limit C13).
    assert!(fixture.held_now().is_empty());
    fixture
        .execution_audit
        .failing
        .store(false, Ordering::SeqCst);
    fixture.person_sends("next", "and now?").await;
    assert!(fixture.contexts_given("next").await.is_empty());
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&app_turn));
}

#[tokio::test]
async fn m13_a_message_whose_submission_task_failed_before_the_agent_was_asked_is_not_sent() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // The submission's own task falls over reading the conversation's
    // record: before the agent was asked to take anything.
    fixture
        .repository
        .verification_panics
        .store(true, Ordering::SeqCst);
    fixture.answer(&review, ALLOW).await;
    let result = task.await.unwrap();
    fixture
        .repository
        .verification_panics
        .store(false, Ordering::SeqCst);
    assert!(
        matches!(result, Err(ConversationError::Unavailable)),
        "{result:?}"
    );
    // Known not to have reached the agent: not sent, not unresolved.
    let phases = fixture.audit.phases();
    assert!(
        matches!(
            phases.last().unwrap(),
            McpAppAuditPhase::MessageNotSent {
                code: ConversationErrorCode::TemporarilyUnavailable,
                ..
            }
        ),
        "{phases:?}"
    );
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
    assert_eq!(fixture.held_now(), ["kept"]);
}

#[tokio::test]
async fn m15_a_message_the_agent_could_not_settle_is_on_record_as_unresolved() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // The agent's own admission task falls over saving it: it cannot say
    // whether it has the message.
    fixture.storage_panics.store(true, Ordering::SeqCst);
    fixture.answer(&review, ALLOW).await;
    let result = task.await.unwrap();
    fixture.storage_panics.store(false, Ordering::SeqCst);
    assert!(
        matches!(
            result,
            Err(ConversationError::Agent(AgentError::SubmissionUnresolved))
        ),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    assert!(
        matches!(
            phases.last().unwrap(),
            McpAppAuditPhase::MessageUnresolved {
                code: ConversationErrorCode::SubmissionUnresolved,
                ..
            }
        ),
        "{phases:?}"
    );
    // Not taken, as far as anyone knows: nothing let go of.
    assert_eq!(fixture.held_now(), ["kept"]);
}

#[tokio::test]
async fn m15_a_message_whose_submission_task_failed_once_the_agent_was_asked_is_unresolved() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // The submission's own task falls over after the agent took it, as the
    // conversation's list entry is written.
    fixture
        .summaries
        .record_panics
        .store(true, Ordering::SeqCst);
    fixture.answer(&review, ALLOW).await;
    let result = task.await.unwrap();
    fixture
        .summaries
        .record_panics
        .store(false, Ordering::SeqCst);
    assert!(
        matches!(result, Err(ConversationError::Unavailable)),
        "{result:?}"
    );
    let phases = fixture.audit.phases();
    assert!(
        matches!(
            phases.last().unwrap(),
            McpAppAuditPhase::MessageUnresolved {
                code: ConversationErrorCode::TemporarilyUnavailable,
                ..
            }
        ),
        "{phases:?}"
    );
}

#[tokio::test]
async fn m16_a_message_whose_step_cannot_be_recorded_is_not_sent_or_shown() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    // Its review's request cannot be recorded: never shown.
    fixture.audit.failing.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.send_message(INSTANCE, "hello").await,
        Err(ConversationError::Audit)
    ));
    assert!(fixture.app_reviews().await.is_empty());
    fixture.audit.failing.store(false, Ordering::SeqCst);
    // The person's allowing cannot be recorded: not sent.
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    let taken = fixture.audit.records.lock().unwrap().len();
    *fixture.audit.failing_after.lock().unwrap() = Some(taken);
    fixture.answer(&review, ALLOW).await;
    assert!(matches!(task.await.unwrap(), Err(ConversationError::Audit)));
    assert_eq!(
        fixture.provider.executions.lock().unwrap().len(),
        executions
    );
}

#[tokio::test]
async fn m16b_a_message_whose_sending_cannot_be_recorded_is_the_agents_and_its_turn_withheld() {
    let fixture = Fixture::new().await;
    let executions = fixture.provider.executions.lock().unwrap().len();
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    // `ApprovalRequested` and `Approved` are written; `MessageSent` is not.
    let taken = fixture.audit.records.lock().unwrap().len();
    *fixture.audit.failing_after.lock().unwrap() = Some(taken + 1);
    fixture.answer(&review, ALLOW).await;
    assert!(matches!(task.await.unwrap(), Err(ConversationError::Audit)));
    assert!(matches!(
        fixture.audit.phases().last(),
        Some(McpAppAuditPhase::Approved { .. })
    ));
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
async fn m18_a_release_past_the_locks_check_finds_the_message_sent() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held_message(INSTANCE, "hello").await;
    let (began, go) = fixture.hold_admission();
    fixture.answer(&review, ALLOW).await;
    // Inside the agent's admission: past the conversation's last check.
    began.notified().await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    let _ = go.send(());
    let execution = task.await.unwrap().unwrap();
    assert_eq!(fixture.given(&execution).await.text_str(), "hello");
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::MessageSent {
            execution_id: execution,
            code: None,
        })
    );
}

#[tokio::test]
async fn an_apps_message_keeps_its_author_across_a_close_and_a_reopening() {
    let fixture = Fixture::new().await;
    let execution = fixture.sent(INSTANCE, "first").await.unwrap();
    fixture.idle().await;
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

#[tokio::test]
async fn a_person_resending_an_apps_turn_as_theirs_is_a_conflict() {
    let fixture = Fixture::new().await;
    let execution = fixture.sent(INSTANCE, "first").await.unwrap();
    fixture.idle().await;
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

// --- mcp.updateModelContext -------------------------------------------------

#[tokio::test]
async fn c2_a_context_from_no_app_another_server_or_a_released_mount_is_refused() {
    let fixture = Fixture::new().await;
    let mut unknown = fixture.app(INSTANCE);
    unknown.tool_id = "not-a-tool-call".into();
    for (app, server) in [(unknown, SERVER), (fixture.app(INSTANCE), "files")] {
        let _ = fixture
            .service
            .update_app_model_context(
                fixture.id.clone(),
                caller("app-context"),
                McpAppContextUpdate {
                    app,
                    server: server.into(),
                    text: Some("x".into()),
                    structured_content_json: None,
                },
            )
            .await;
    }
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(
        refused(fixture.update_context(INSTANCE, Some("x"), None).await),
        McpAppError::Cancelled
    );
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(
        records
            .iter()
            .map(|record| record.phase.clone())
            .collect::<Vec<_>>(),
        [
            McpAppAuditPhase::Refused(McpAppCode::AppUnknown),
            McpAppAuditPhase::Refused(McpAppCode::ServerMismatch),
            McpAppAuditPhase::Refused(McpAppCode::Cancelled),
        ]
    );
    assert_eq!(records[2].initiator, McpAppInitiator::System);
    assert!(fixture.held_now().is_empty());
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
        }
    );
    assert_eq!(
        phases[1],
        McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge)
    );
    assert!(phases[2..]
        .iter()
        .all(|phase| phase == &McpAppAuditPhase::Refused(McpAppCode::InvalidRequest)));
    assert_eq!(phases.len(), 6);
    // What a refusal could not replace is still what is held.
    fixture.person_turn("next").await;
    assert_eq!(fixture.contexts_given("next").await, [Some(at_bound)]);
}

#[tokio::test]
async fn c5_c9_the_latest_context_goes_with_the_next_idle_message_once_and_names_its_update() {
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
    let latest = fixture.audit.records.lock().unwrap()[1].call_id.clone();
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
    // It names the update that gave it, as that update's record does.
    assert_eq!(contexts[0].update_id(), latest);
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
    // Let go of once that message was saved: the message after carries none.
    assert!(fixture.held_now().is_empty());
    fixture.person_turn("after").await;
    assert!(fixture.given("after").await.app_model_context().is_empty());
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::ContextHeld { bytes: 5 },
            McpAppAuditPhase::ContextHeld {
                bytes: 3 + r#"{"month": 5, "month": 6}"#.len(),
            },
        ]
    );
}

#[tokio::test]
async fn c9_an_apps_own_message_carries_the_context_too() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("selected: row 3"), None)
        .await
        .unwrap();
    let execution = fixture.sent(INSTANCE, "explain this row").await.unwrap();
    assert_eq!(
        fixture.contexts_given(&execution).await,
        [Some("selected: row 3".to_owned())]
    );
    assert!(fixture.held_now().is_empty());
}

#[tokio::test]
async fn c9_a_newer_update_given_between_the_read_and_the_enqueue_stays() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("old"), None)
        .await
        .unwrap();
    fixture
        .update_context(OTHER_INSTANCE, Some("other"), None)
        .await
        .unwrap();
    // The person's message has read what is held, and is being admitted.
    let (began, go) = fixture.hold_admission();
    let sending = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let caller = fixture.caller("next");
        tokio::spawn(async move {
            service
                .submit(
                    id,
                    caller,
                    "next".into(),
                    SubmittedMessage {
                        text: "and now?".into(),
                        ..SubmittedMessage::default()
                    },
                    SubmissionMode::Queue,
                )
                .await
        })
    };
    began.notified().await;
    // The mount gives a newer context meanwhile.
    fixture
        .update_context(INSTANCE, Some("newer"), None)
        .await
        .unwrap();
    let _ = go.send(());
    sending.await.unwrap().unwrap();
    // The message carried what it read; exactly those are let go of, and
    // the newer one stays for the next.
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("old".to_owned()), Some("other".to_owned())]
    );
    assert_eq!(fixture.held_now(), ["newer"]);
    fixture.idle().await;
    fixture.person_turn("after").await;
    assert_eq!(
        fixture.contexts_given("after").await,
        [Some("newer".to_owned())]
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
    assert_eq!(fixture.held_now().len(), MAX_HELD_CONTEXTS);
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
async fn c7_an_update_with_neither_part_or_an_empty_text_clears_what_the_mount_held() {
    let fixture = Fixture::new().await;
    for clear in [Some(""), None] {
        fixture
            .update_context(INSTANCE, Some("x"), None)
            .await
            .unwrap();
        fixture.update_context(INSTANCE, clear, None).await.unwrap();
        assert!(fixture.held_now().is_empty());
    }
    fixture.person_turn("next").await;
    assert!(fixture.given("next").await.app_model_context().is_empty());
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::ContextHeld { bytes: 1 },
            McpAppAuditPhase::ContextCleared,
            McpAppAuditPhase::ContextHeld { bytes: 1 },
            McpAppAuditPhase::ContextCleared,
        ]
    );
}

#[tokio::test]
async fn c8_two_updates_at_once_are_recorded_in_the_order_they_are_held() {
    // Another mount's update waits on this one's: one lock per conversation.
    let fixture = Fixture::new().await;
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    let earlier = updating(&fixture, INSTANCE, Some("A"));
    hold.waiting.notified().await;
    let later = updating(&fixture, OTHER_INSTANCE, Some("B"));
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert!(
        fixture.audit.phases().is_empty(),
        "an update was recorded while another's was being recorded"
    );
    hold.go.add_permits(1);
    earlier.await.unwrap().unwrap();
    later.await.unwrap().unwrap();
    assert_eq!(fixture.held_now(), ["A", "B"]);

    // The same mount: the one recorded last is the one that stands.
    for (first, second, carried) in [
        (Some("A"), None, Vec::new()),
        (None, Some("B"), vec![Some("B".to_owned())]),
    ] {
        let fixture = Fixture::new().await;
        let hold = Hold::default();
        *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
        let earlier = updating(&fixture, INSTANCE, first);
        hold.waiting.notified().await;
        let later = updating(&fixture, INSTANCE, second);
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert!(fixture.audit.phases().is_empty());
        hold.go.add_permits(1);
        earlier.await.unwrap().unwrap();
        later.await.unwrap().unwrap();
        let phases = fixture.audit.phases();
        assert_eq!(
            matches!(phases[0], McpAppAuditPhase::ContextHeld { .. }),
            first.is_some()
        );
        assert_eq!(
            matches!(phases[1], McpAppAuditPhase::ContextHeld { .. }),
            second.is_some()
        );
        fixture.person_turn("next").await;
        assert_eq!(fixture.contexts_given("next").await, carried);
    }
}

#[tokio::test]
async fn c10_a_message_queued_behind_a_turn_carries_no_context_and_leaves_it_held() {
    let fixture = Fixture::new().await;
    let finish = running(&fixture, "running").await;
    fixture
        .update_context(INSTANCE, Some("kept"), None)
        .await
        .unwrap();
    // Queued behind it: it carries none, so its removal or a reordering
    // can lose or invert nothing.
    fixture.person_sends("queued", "and then").await;
    assert_eq!(fixture.held_now(), ["kept"]);
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
async fn c10_a_message_steered_into_a_turn_carries_no_context() {
    let fixture = Fixture::new().await;
    let finish = running(&fixture, "running").await;
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
    assert_eq!(fixture.held_now(), ["kept"]);
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
async fn c11_a_refused_submission_lets_go_of_nothing() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    // The agent refuses it as it is admitted: it cannot save it.
    fixture.storage_refuses.store(true, Ordering::SeqCst);
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
    fixture.storage_refuses.store(false, Ordering::SeqCst);
    assert!(
        matches!(
            refused,
            Err(ConversationError::Agent(AgentError::Storage(_)))
        ),
        "{refused:?}"
    );
    assert_eq!(fixture.held_now(), ["ctx"]);
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
    assert_eq!(fixture.held_now(), ["ctx"]);
    // Held, it goes with the next.
    fixture.person_sends("next", "again").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("ctx".to_owned())]
    );
}

#[tokio::test]
async fn c12_a_retry_of_a_message_the_agent_has_carries_what_its_record_holds() {
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
    // The same submission again: the agent's to settle, and no conflict —
    // it carries what was saved, not what is held now.
    fixture.person_sends("once", "and now?").await;
    assert_eq!(
        fixture.contexts_given("once").await,
        [Some("first".to_owned())]
    );
    // What was held since is still held, for the next new message.
    assert_eq!(fixture.held_now(), ["second"]);
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("second".to_owned())]
    );
}

#[tokio::test]
async fn c13_a_context_carried_by_a_turn_that_then_fails_is_lost() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("ctx"), None)
        .await
        .unwrap();
    // Admitted and saved, then failed as it was prepared, before its prompt
    // went: the agent never saw the context.
    *fixture.provider.prepare_failure.lock().unwrap() = Some(AgentError::Protocol("no".into()));
    fixture.person_sends("p1", "first").await;
    settled(&fixture, "p1").await;
    assert!(!fixture
        .provider
        .executions
        .lock()
        .unwrap()
        .contains(&"p1".to_owned()));
    // Let go of at admission all the same: lost, and the app may give it
    // again (recorded limit).
    assert!(fixture.held_now().is_empty());
    fixture.person_sends("p2", "second").await;
    assert!(fixture.contexts_given("p2").await.is_empty());
}

#[tokio::test]
async fn c14_a_mounts_release_drops_its_context_unsent() {
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
    assert_eq!(fixture.held_now(), ["other"]);
    // On record, against the update that held it, by the releaser.
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::Released, person_by("release"))]
    );
    // Released again: nothing more to drop, nothing more on record.
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(drops(&fixture).len(), 1);
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("other".to_owned())]
    );
}

#[tokio::test]
async fn c14_a_drop_that_cannot_be_recorded_is_dropped_and_the_release_says_so() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("mine"), None)
        .await
        .unwrap();
    let (waiting, _review) = fixture.held_message(INSTANCE, "hello").await;
    fixture.audit.failing.store(true, Ordering::SeqCst);
    let released = fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await;
    assert!(
        matches!(released, Err(ConversationError::Audit)),
        "{released:?}"
    );
    // Released all the same: the context dropped, the review withdrawn, and
    // the mount refused from now on.
    assert!(fixture.held_now().is_empty());
    assert!(fixture.app_reviews().await.is_empty());
    assert!(waiting.await.unwrap().is_err());
    fixture.audit.failing.store(false, Ordering::SeqCst);
    assert_eq!(
        refused(fixture.update_context(INSTANCE, Some("again"), None).await),
        McpAppError::Cancelled
    );
}

#[tokio::test]
async fn c15_the_openings_end_drops_every_context_unsent() {
    // A close.
    let fixture = Fixture::new().await;
    fixture
        .update_context(OTHER_INSTANCE, Some("again"), None)
        .await
        .unwrap();
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    assert!(fixture.held_now().is_empty());
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::ConversationEnded, person_by("close"))]
    );
    fixture.person_turn("reopened").await;
    assert!(fixture
        .given("reopened")
        .await
        .app_model_context()
        .is_empty());

    // The desktop stopping every agent, and a gateway stop.
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    fixture.service.stop_active_agents().await.unwrap();
    assert!(fixture.held_now().is_empty());
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::ConversationEnded, McpAppInitiator::System)]
    );
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    fixture.service.shutdown().await.unwrap();
    assert!(fixture.held_now().is_empty());
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::ConversationEnded, McpAppInitiator::System)]
    );

    // A delete.
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    // The fixture names no eraser for the agent's own session, so the delete
    // happens and says its erasure is unfinished; its apps end either way.
    let deleted = fixture
        .service
        .delete(fixture.id.clone(), caller("delete"))
        .await;
    assert!(
        matches!(
            deleted,
            Ok(_) | Err(ConversationError::DeletionIncomplete(_))
        ),
        "{deleted:?}"
    );
    assert!(fixture.held_now().is_empty());
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::ConversationEnded, person_by("delete"))]
    );

    // An end whose drop cannot be recorded: dropped all the same, and the
    // close goes on.
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("x"), None)
        .await
        .unwrap();
    fixture.audit.failing.store(true, Ordering::SeqCst);
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    assert!(fixture.held_now().is_empty());
    assert!(drops(&fixture).is_empty());
}

#[tokio::test]
async fn c16_an_update_that_cannot_be_recorded_changes_nothing() {
    let fixture = Fixture::new().await;
    fixture
        .update_context(INSTANCE, Some("on record"), None)
        .await
        .unwrap();
    fixture.audit.failing.store(true, Ordering::SeqCst);
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
    assert!(matches!(
        fixture
            .update_context(OTHER_INSTANCE, Some("unrecorded"), None)
            .await,
        Err(ConversationError::Audit)
    ));
    fixture.audit.failing.store(false, Ordering::SeqCst);
    assert_eq!(fixture.held_now(), ["on record"]);
    fixture.person_turn("next").await;
    assert_eq!(
        fixture.contexts_given("next").await,
        [Some("on record".to_owned())]
    );
}

#[tokio::test]
async fn c17_a_release_between_the_record_and_the_hold_holds_nothing() {
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
    // On record, and answered applied: the release came after it. That it
    // was never held is on record too, as the system's.
    update.await.unwrap().unwrap();
    assert_eq!(
        drops(&fixture),
        [(ContextDrop::NotHeld, McpAppInitiator::System)]
    );
    assert!(fixture.held_now().is_empty());
    fixture.person_turn("next").await;
    assert!(fixture.given("next").await.app_model_context().is_empty());
}

#[test]
fn a_message_carries_as_many_contexts_as_a_conversation_holds() {
    // One owner: the conversation holds no more than one message carries.
    assert_eq!(MAX_HELD_CONTEXTS, UserMessage::MAX_APP_MODEL_CONTEXTS);
}
