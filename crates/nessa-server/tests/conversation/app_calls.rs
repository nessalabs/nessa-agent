//! An MCP App's calls through the conversation service (#348): one test per
//! row of the "MCP App call" state table — each policy refusal, a call sent
//! at once, a destructive call allowed, denied, cancelled, expired and
//! withdrawn (by its request going, its mount being released, and its
//! conversation ending), a server's failures, a resource held behind a
//! ticket — and the audit each leaves.
use super::*;
use crate::app_call_test_support::{caller, Fixture, Hold, INSTANCE, OTHER_INSTANCE, SERVER, URI};
use crate::conversation::application::app_reviews::{
    ALLOW, DENY, MAX_APP_REVIEW_BYTES, MAX_OPEN_APP_REVIEWS,
};
use crate::conversation::application::mcp_apps::TicketEnd;
use crate::conversation::application::DeletionFailures;
use crate::mcp_servers::domain::MAX_APP_ARGUMENTS_BYTES;
use nessa_auth::domain::PrincipalId;
use nessa_protocol::conversation::projection::{
    review_entry, MAX_LISTED_REVIEWS, MAX_REVIEW_ENTRY_BYTES, MAX_REVIEW_ENTRY_TOOL_NAME_BYTES,
    MAX_VIEW_BYTES,
};
use nessa_protocol::conversation::view::{
    ConversationMessage, ConversationPermissionOrigin, ConversationReviewEntry,
    ConversationReviewKind, ConversationTranscriptState,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::permissions::PermissionCaller;
use nessa_sdk::domain::agent_execution::permissions::MAX_OPEN_REVIEWS;
use nessa_sdk::domain::mcp_apps::{ToolHints, ToolUi, UiCsp, UiResource, UiVisibility};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The person behind `caller(action)`, by that request.
fn person_by(action: &str) -> McpAppInitiator {
    McpAppInitiator::Person {
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        request_id: action.into(),
    }
}

fn refused(result: Result<impl std::fmt::Debug, ConversationError>) -> McpAppError {
    match result {
        Err(ConversationError::McpApp(error)) => error,
        other => panic!("expected an app refusal, got {other:?}"),
    }
}

fn is_permission(phase: &McpAppAuditPhase) -> Option<&str> {
    match phase {
        McpAppAuditPhase::ApprovalRequested { permission_id }
        | McpAppAuditPhase::Approved { permission_id }
        | McpAppAuditPhase::Denied { permission_id }
        | McpAppAuditPhase::Expired { permission_id }
        | McpAppAuditPhase::Withdrawn { permission_id, .. } => Some(permission_id),
        _ => None,
    }
}

#[tokio::test]
async fn a_tool_that_only_reads_is_called_at_once_and_its_answer_returned_verbatim() {
    let fixture = Fixture::new().await;
    fixture.apps.answers.lock().unwrap().push(Ok(
        json!({"content": [{"type": "text", "text": "3 rows"}], "isError": true}),
    ));
    let answer = fixture
        .call_tool(fixture.call("read_rows", Some("{\"table\":\"t\"}")))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&answer).unwrap(),
        json!({"content": [{"type": "text", "text": "3 rows"}], "isError": true})
    );
    assert_eq!(
        fixture.apps.calls.lock().unwrap()[0],
        ("read_rows".into(), Some(json!({"table": "t"})))
    );
    let bytes = answer.len();
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::Completed(McpAppOutcome::Answered {
                is_error: true,
                bytes
            }),
        ]
    );
    // Every step of one call shares the gateway's call id, and names the
    // app, its mount, its ask, and who it acted for.
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].call_id, records[1].call_id);
    assert_eq!(records[0].app, fixture.app(INSTANCE));
    assert_eq!(records[0].request_id, "app-call");
    assert_eq!(
        records[0].ask,
        McpAppAsk::CallTool {
            server: SERVER.into(),
            tool: "read_rows".into()
        }
    );
    assert_eq!(
        records[0].initiator,
        McpAppInitiator::App {
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "panel".into()
        }
    );
    // A second call is a second call id, even under the same request id.
    fixture
        .call_tool(fixture.call("read_rows", None))
        .await
        .unwrap();
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_ne!(records[2].call_id, records[0].call_id);
}

#[tokio::test]
async fn each_policy_refusal_is_its_code_on_record_and_nothing_is_sent() {
    let fixture = Fixture::new().await;
    fixture.apps.list(
        "model_only",
        Some(UiVisibility::new(true, false)),
        ToolHints::new(Some(true), None),
    );
    // Model-only with no UI of its own is refused all the same (#412).
    fixture.apps.list_declared(
        "model_only_undrawn",
        ToolUi::new(None, UiVisibility::new(true, false)),
        ToolHints::new(Some(true), None),
    );
    // No one's, as a `visibility` that cannot be read is read (#412): refused
    // even with a UI of its own.
    fixture.apps.list(
        "unreadable",
        Some(UiVisibility::new(false, false)),
        ToolHints::new(Some(true), None),
    );
    let not_this_app = McpAppCall {
        app: McpAppRef {
            tool_id: "no-such-call".into(),
            ..fixture.app(INSTANCE)
        },
        ..fixture.call("read_rows", None)
    };
    let another_server = McpAppCall {
        server: "files".into(),
        ..fixture.call("read_rows", None)
    };
    let past_the_bound = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_ARGUMENTS_BYTES - 7));
    assert_eq!(past_the_bound.len(), MAX_APP_ARGUMENTS_BYTES + 1);
    for (call, expected) in [
        (not_this_app, McpAppError::AppUnknown),
        (another_server, McpAppError::ServerMismatch),
        (fixture.call("not_listed", None), McpAppError::ToolNotForApp),
        (fixture.call("model_only", None), McpAppError::ToolNotForApp),
        (
            fixture.call("model_only_undrawn", None),
            McpAppError::ToolNotForApp,
        ),
        (fixture.call("unreadable", None), McpAppError::ToolNotForApp),
        (
            fixture.call("read_rows", Some(&past_the_bound)),
            McpAppError::RequestTooLarge,
        ),
    ] {
        assert_eq!(refused(fixture.call_tool(call).await), expected);
        assert_eq!(
            fixture.audit.phases().last(),
            Some(&McpAppAuditPhase::Refused(expected.code()))
        );
    }
    // Not a JSON object: refused as invalid, on record too.
    assert!(matches!(
        fixture
            .call_tool(fixture.call("read_rows", Some("[1]")))
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused(
            ConversationErrorCode::InvalidRequest
        ))
    );
    assert_eq!(fixture.apps.calls(), 0);
    // Exactly at the bound is taken.
    let at_the_bound = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_ARGUMENTS_BYTES - 8));
    assert_eq!(at_the_bound.len(), MAX_APP_ARGUMENTS_BYTES);
    fixture
        .call_tool(fixture.call("read_rows", Some(&at_the_bound)))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_destructive_call_waits_on_the_persons_review_and_is_sent_once_allowed() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture
        .held(fixture.call("delete_rows", Some("{\"id\":1}")))
        .await;
    // Shown beside the agent's, naming the app's tool and what it asked.
    assert_eq!(review.execution_id, fixture.execution_id);
    assert_eq!(review.tool_id, fixture.tool_id);
    assert_eq!(review.arguments_json, "{\"id\":1}");
    assert_eq!(review.tool_name, "delete_rows");
    assert_eq!(
        review.origin,
        ConversationPermissionOrigin::App {
            server: SERVER.into(),
            tool: "delete_rows".into()
        }
    );
    assert_eq!(fixture.apps.calls(), 0, "nothing is sent while it waits");
    fixture.answer(&review, ALLOW).await;
    task.await.unwrap().unwrap();
    assert_eq!(fixture.apps.calls(), 1);
    assert!(fixture.app_reviews().await.is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    let phases: Vec<_> = records.iter().map(|record| &record.phase).collect();
    assert!(matches!(
        phases[..],
        [
            McpAppAuditPhase::ApprovalRequested { .. },
            McpAppAuditPhase::Approved { .. },
            McpAppAuditPhase::Completed(McpAppOutcome::Answered { .. }),
        ]
    ));
    assert_eq!(
        is_permission(phases[0]),
        Some(review.permission_id.as_str())
    );
    // The approval is the person's, answering by its own request.
    assert_eq!(
        records[1].initiator,
        McpAppInitiator::Person {
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "panel".into(),
            request_id: "answer".into()
        }
    );
    // Answered again: stale, and nothing more happens.
    let again = fixture
        .service
        .answer(
            fixture.id.clone(),
            caller("again"),
            review.execution_id.clone(),
            review.permission_id.clone(),
            DENY.into(),
            PermissionCaller::Owner,
        )
        .await;
    // Ended, the identity names no app review: the agent's to answer, which
    // answers it stale itself.
    assert!(matches!(
        again,
        Err(ConversationError::PermissionAnswer {
            error: AgentError::StalePermission,
            ..
        })
    ));
    assert_eq!(fixture.apps.calls(), 1);
}

#[tokio::test]
async fn a_denied_or_cancelled_review_ends_its_call_denied_and_nothing_is_sent() {
    let fixture = Fixture::new().await;
    let (denied, review) = fixture.held(fixture.call("delete_rows", None)).await;
    fixture.answer(&review, DENY).await;
    assert_eq!(refused(denied.await.unwrap()), McpAppError::ApprovalDenied);
    let (cancelled, review) = fixture.held(fixture.call("delete_rows", None)).await;
    fixture
        .service
        .cancel_permission(
            fixture.id.clone(),
            caller("cancel"),
            review.execution_id.clone(),
            review.permission_id.clone(),
            "no".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        refused(cancelled.await.unwrap()),
        McpAppError::ApprovalDenied
    );
    assert_eq!(fixture.apps.calls(), 0);
    let denials = fixture
        .audit
        .phases()
        .into_iter()
        .filter(|phase| matches!(phase, McpAppAuditPhase::Denied { .. }))
        .count();
    assert_eq!(denials, 2);
}

#[tokio::test]
async fn a_review_nobody_answers_expires_and_its_call_is_refused_expired() {
    let fixture = Fixture::new().await;
    let (task, _review) = fixture.held(fixture.call("delete_rows", None)).await;
    tokio::time::pause();
    tokio::time::advance(APP_REVIEW_DEADLINE + Duration::from_secs(1)).await;
    tokio::time::resume();
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalExpired);
    assert_eq!(fixture.apps.calls(), 0);
    assert!(fixture.app_reviews().await.is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    let expired = records.last().unwrap();
    assert!(matches!(expired.phase, McpAppAuditPhase::Expired { .. }));
    assert_eq!(expired.initiator, McpAppInitiator::System);
}

#[tokio::test]
async fn a_call_whose_caller_went_withdraws_its_review_on_record() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.app_reviews().await.is_empty()
            || !matches!(
                fixture.audit.phases().last(),
                Some(McpAppAuditPhase::Withdrawn { .. })
            )
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id.clone(),
            cause: McpAppWithdrawal::RequestCancelled
        })
    );
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn releasing_one_mount_cancels_only_its_own_calls_and_lets_go_of_its_resources() {
    let fixture = Fixture::new().await;
    let (mine, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let (other, other_review) = fixture
        .held(McpAppCall {
            app: fixture.app(OTHER_INSTANCE),
            ..fixture.call("delete_rows", None)
        })
        .await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(mine.await.unwrap()), McpAppError::Cancelled);
    // The release is the releaser's: the caller of `mcp.releaseApp`, by its
    // own request.
    let releaser = person_by("release");
    assert_eq!(
        fixture.tickets.released_apps.lock().unwrap().clone(),
        [(fixture.app(INSTANCE), releaser.clone())]
    );
    // The other mount still waits, and is answered as it is answered.
    let left = fixture.app_reviews().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].permission_id, other_review.permission_id);
    fixture.answer(&other_review, ALLOW).await;
    other.await.unwrap().unwrap();
    let torn_down: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| {
            matches!(
                record.phase,
                McpAppAuditPhase::Withdrawn {
                    cause: McpAppWithdrawal::AppTornDown,
                    ..
                }
            )
        })
        .map(|record| record.initiator.clone())
        .collect();
    assert_eq!(torn_down, [releaser]);
    // Releasing it again, or a mount with nothing open, is nothing.
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
}

#[tokio::test]
async fn closing_the_conversation_cancels_every_waiting_call_and_its_resources() {
    let fixture = Fixture::new().await;
    let (first, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let (second, _) = fixture
        .held(McpAppCall {
            app: fixture.app(OTHER_INSTANCE),
            ..fixture.call("delete_rows", None)
        })
        .await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    for task in [first, second] {
        assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    }
    // Ended by the person who closed it, by their own request.
    let closer = person_by("close");
    assert_eq!(
        *fixture.tickets.released_conversations.lock().unwrap(),
        std::slice::from_ref(&closer)
    );
    let withdrawals: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| {
            matches!(
                record.phase,
                McpAppAuditPhase::Withdrawn {
                    cause: McpAppWithdrawal::ConversationEnded,
                    ..
                }
            )
        })
        .map(|record| record.initiator.clone())
        .collect();
    assert_eq!(withdrawals, [closer.clone(), closer]);
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn past_the_open_review_cap_a_destructive_call_is_refused_unavailable() {
    let fixture = Fixture::new().await;
    let mut held = Vec::new();
    for _ in 0..MAX_OPEN_APP_REVIEWS {
        held.push(fixture.held(fixture.call("delete_rows", None)).await.0);
    }
    assert!(matches!(
        fixture.call_tool(fixture.call("delete_rows", None)).await,
        Err(ConversationError::Unavailable)
    ));
    for task in held {
        task.abort();
    }
}

#[tokio::test]
async fn a_servers_failure_is_its_code_and_on_record_as_completed() {
    let fixture = Fixture::new().await;
    let huge = json!({"content": [{"type": "text", "text": "x".repeat(MAX_APP_RESULT_BYTES)}]});
    for (answer, expected) in [
        (
            Err(McpAppFailure::Remote {
                code: -32602,
                message: "bad".into(),
            }),
            McpAppError::Remote(Some((-32602, "bad".into()))),
        ),
        (Err(McpAppFailure::TimedOut), McpAppError::TimedOut),
        (
            Err(McpAppFailure::SessionEnded),
            McpAppError::SessionUnavailable,
        ),
        (Err(McpAppFailure::Malformed), McpAppError::Remote(None)),
        (Ok(huge), McpAppError::ResultTooLarge),
    ] {
        fixture.apps.answers.lock().unwrap().push(answer);
        let code = expected.code();
        assert_eq!(
            refused(fixture.call_tool(fixture.call("read_rows", None)).await),
            expected
        );
        assert_eq!(
            fixture.audit.phases().last(),
            Some(&McpAppAuditPhase::Completed(McpAppOutcome::Failed(code)))
        );
    }
}

#[tokio::test]
async fn a_step_that_cannot_be_recorded_is_not_taken() {
    let fixture = Fixture::new().await;
    fixture.audit.failing.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.call_tool(fixture.call("read_rows", None)).await,
        Err(ConversationError::Audit)
    ));
    assert!(matches!(
        fixture.call_tool(fixture.call("delete_rows", None)).await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(fixture.apps.calls(), 0);
    assert!(fixture.app_reviews().await.is_empty(), "no review shown");
}

#[tokio::test]
async fn a_resource_is_read_once_and_held_behind_a_ticket_on_record() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p>chart</p>".into(),
        UiCsp::new(
            vec![],
            vec!["https://cdn.example.com".into()],
            vec![],
            vec![],
        )
        .unwrap(),
        UiPermissions::default(),
        None,
        Some(true),
    )
    .unwrap()));
    let read = McpAppRead {
        app: fixture.app(INSTANCE),
        server: SERVER.into(),
        uri: URI.into(),
    };
    let resource = fixture
        .service
        .read_app_resource(fixture.id.clone(), caller("read-resource"), read.clone())
        .await
        .unwrap();
    assert_eq!(resource.size, "<p>chart</p>".len());
    assert_eq!(resource.sha256, hex(b"<p>chart</p>"));
    assert_eq!(resource.csp.resource_domains().len(), 1);
    assert_eq!(resource.prefers_border, Some(true));
    let issued = fixture.tickets.issued.lock().unwrap().clone();
    assert_eq!(&*issued[0].bytes, b"<p>chart</p>");
    assert_eq!(issued[0].app(), &fixture.app(INSTANCE));
    let phases = fixture.audit.phases();
    assert_eq!(phases[0], McpAppAuditPhase::Admitted);
    assert_eq!(
        phases[2],
        McpAppAuditPhase::TicketIssued {
            // Only its digest is ever on record.
            ticket_digest: ResourceTicketDigest::of(resource.ticket.as_bytes()).to_hex(),
            size: resource.size,
            sha256: resource.sha256.clone()
        }
    );
    // Another server's: refused. Not an app's HTML: app_unknown, on record.
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(
                    fixture.id.clone(),
                    caller("read-resource"),
                    McpAppRead {
                        server: "files".into(),
                        ..read.clone()
                    }
                )
                .await
        ),
        McpAppError::ServerMismatch
    );
    *fixture.apps.resource.lock().unwrap() = Some(Err(McpAppFailure::NotAnApp));
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(fixture.id.clone(), caller("read-resource"), read.clone())
                .await
        ),
        McpAppError::AppUnknown
    );
    // No room to hold it: refused unavailable, on record, nothing issued.
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p/>".into(),
        UiCsp::default(),
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()));
    fixture.tickets.full.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .service
            .read_app_resource(fixture.id.clone(), caller("read-resource"), read)
            .await,
        Err(ConversationError::Unavailable)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Completed(McpAppOutcome::Failed(
            ConversationErrorCode::TemporarilyUnavailable
        )))
    );
}

#[tokio::test]
async fn calls_running_are_bounded_across_callers_until_each_task_ends() {
    // A caller that goes leaves its call running on its own task; the bound
    // counts that task, not the caller, so coming back cannot start more.
    let fixture = Fixture::new().await;
    fixture.apps.hold.store(true, Ordering::SeqCst);
    let mut callers = Vec::new();
    for _ in 0..MAX_APP_CALLS {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let call = fixture.call("read_rows", None);
        callers.push(tokio::spawn(async move {
            service.call_app_tool(id, caller("held"), call).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.apps.calls() < MAX_APP_CALLS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for caller in &callers {
        caller.abort();
    }
    for caller in callers {
        let _ = caller.await;
    }
    // Every caller went; every call is still running, and still counted. A
    // bound that let it in would hold it on the server: a deadline, so that
    // fails rather than hangs.
    let again = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.call_tool(fixture.call("read_rows", None)),
    )
    .await
    .expect("refused at once, not admitted and held");
    assert!(matches!(again, Err(ConversationError::Unavailable)));
    fixture.apps.hold.store(false, Ordering::SeqCst);
    fixture.apps.gate.0.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .audit
            .phases()
            .iter()
            .filter(|phase| matches!(phase, McpAppAuditPhase::Completed(_)))
            .count()
            < MAX_APP_CALLS
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Each finished on its own and is on record; then there is room again.
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .call_tool(fixture.call("read_rows", None))
            .await
            .is_err()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_ticket_whose_issue_cannot_be_recorded_is_discarded_and_never_handed_out() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p/>".into(),
        UiCsp::default(),
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()));
    // Admitted and completed are recorded; the issue is not.
    *fixture.audit.failing_after.lock().unwrap() = Some(2);
    let read = fixture
        .service
        .read_app_resource(
            fixture.id.clone(),
            caller("read-resource"),
            McpAppRead {
                app: fixture.app(INSTANCE),
                server: SERVER.into(),
                uri: URI.into(),
            },
        )
        .await;
    assert!(matches!(read, Err(ConversationError::Audit)));
    assert_eq!(fixture.tickets.issued.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.tickets.discarded.lock().unwrap().clone(),
        ["t".repeat(43)]
    );
}

#[tokio::test]
async fn a_view_holding_an_app_review_has_a_revision_of_its_own() {
    // A window holding a revision holds what that revision showed: a review
    // opening or ending changes it, though nothing in the transcript did.
    let fixture = Fixture::new().await;
    let revision = || async {
        fixture
            .service
            .read(fixture.id.clone(), caller("read"))
            .await
            .unwrap()
            .revision
    };
    let before = revision().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    let during = revision().await;
    assert_ne!(during, before);
    assert_eq!(
        revision().await,
        during,
        "the same reviews, the same revision"
    );
    fixture.answer(&review, DENY).await;
    let _ = task.await;
    let after = revision().await;
    assert_ne!(after, during);
    assert_eq!(after, before);
}

#[tokio::test]
async fn a_released_mount_is_admitted_nothing_and_nothing_reaches_its_server() {
    // Released: its later calls of every kind — destructive or not, a tool
    // or a resource — are refused before anything is asked of the server,
    // on record as the system's, the release being the releaser's.
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    for tool in ["delete_rows", "read_rows"] {
        assert_eq!(
            refused(fixture.call_tool(fixture.call(tool, None)).await),
            McpAppError::Cancelled
        );
    }
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
                .await
        ),
        McpAppError::Cancelled
    );
    assert!(fixture.app_reviews().await.is_empty());
    assert_eq!(fixture.apps.calls(), 0);
    assert_eq!(fixture.apps.reads.load(Ordering::SeqCst), 0);
    let refusals: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|record| (record.phase.clone(), record.initiator.clone()))
        .collect();
    let refused_so = (
        McpAppAuditPhase::Refused(ConversationErrorCode::McpCancelled),
        McpAppInitiator::System,
    );
    assert_eq!(refusals, vec![refused_so; 3]);
}

fn page(html: &str) -> UiResource {
    UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        html.into(),
        UiCsp::default(),
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()
}

fn read(fixture: &Fixture, instance: &str) -> McpAppRead {
    McpAppRead {
        app: fixture.app(instance),
        server: SERVER.into(),
        uri: URI.into(),
    }
}

#[tokio::test]
async fn a_resource_read_when_its_conversation_closes_is_never_held() {
    // The read is with the server when the conversation closes: nothing is
    // held for it then. It must not be held when the read comes back.
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    fixture.apps.hold.store(true, Ordering::SeqCst);
    let reading = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let read = read(&fixture, INSTANCE);
        tokio::spawn(async move { service.read_app_resource(id, caller("read"), read).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.apps.reads.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    fixture.apps.gate.0.add_permits(1);
    assert_eq!(refused(reading.await.unwrap()), McpAppError::Cancelled);
    assert!(fixture.tickets.issued.lock().unwrap().is_empty());
    // By the close, another command: the system's.
    let last = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        (last.phase, last.initiator),
        (
            McpAppAuditPhase::Completed(McpAppOutcome::Failed(ConversationErrorCode::McpCancelled)),
            McpAppInitiator::System
        )
    );
}

#[tokio::test]
async fn a_released_mount_is_issued_no_ticket() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    let refused_read = fixture
        .service
        .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
        .await;
    assert_eq!(refused(refused_read), McpAppError::Cancelled);
    assert!(fixture.tickets.issued.lock().unwrap().is_empty());
    // Another mount of the same tool call is its own.
    fixture
        .service
        .read_app_resource(
            fixture.id.clone(),
            caller("read"),
            read(&fixture, OTHER_INSTANCE),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_ticket_let_go_while_its_issue_was_recorded_ends_on_record_after_it() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    let releaser = person_by("release");
    *fixture.tickets.ended_first.lock().unwrap() = Some((TicketEnd::AppReleased, releaser.clone()));
    let refused_read = fixture
        .service
        .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
        .await;
    assert_eq!(refused(refused_read), McpAppError::Cancelled);
    let records = fixture.audit.records.lock().unwrap().clone();
    let phases: Vec<_> = records.iter().map(|record| &record.phase).collect();
    assert!(matches!(
        phases[..],
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::Completed(McpAppOutcome::Answered { .. }),
            McpAppAuditPhase::TicketIssued { .. },
            McpAppAuditPhase::TicketEnded {
                cause: TicketEnd::AppReleased,
                ..
            },
        ]
    ));
    // Its end, by whoever ended it, after its issue.
    assert_eq!(records[3].initiator, releaser);
    assert!(fixture.tickets.activated.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_allowed_call_whose_approval_cannot_be_recorded_is_not_made() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    // `ApprovalRequested` is on record; `Approved` will not be.
    *fixture.audit.failing_after.lock().unwrap() = Some(1);
    fixture.answer(&review, ALLOW).await;
    assert!(matches!(task.await.unwrap(), Err(ConversationError::Audit)));
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn a_call_made_whose_answer_cannot_be_recorded_is_withheld() {
    // Sent, so made: the record of its end is what failed, and the answer is
    // withheld rather than reported unrecorded.
    let fixture = Fixture::new().await;
    *fixture.audit.failing_after.lock().unwrap() = Some(1);
    assert!(matches!(
        fixture.call_tool(fixture.call("read_rows", None)).await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(fixture.apps.calls(), 1);
}

#[tokio::test]
async fn a_tool_changed_during_its_review_is_not_called_once_allowed() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    // While the person decides, the server lists it for the model only.
    fixture.apps.list(
        "delete_rows",
        Some(UiVisibility::new(true, false)),
        ToolHints::new(None, None),
    );
    fixture.answer(&review, ALLOW).await;
    assert_eq!(refused(task.await.unwrap()), McpAppError::ToolNotForApp);
    assert_eq!(fixture.apps.calls(), 0);
    let phases = fixture.audit.phases();
    assert!(matches!(
        phases[phases.len() - 2..],
        [
            McpAppAuditPhase::Approved { .. },
            McpAppAuditPhase::Refused(ConversationErrorCode::McpToolNotForApp)
        ]
    ));
}

#[tokio::test]
async fn what_the_person_is_shown_is_what_is_sent() {
    // A duplicate key, and a number past what JSON numbers keep: the review
    // shows the arguments as they will be sent, not as the app wrote them.
    let fixture = Fixture::new().await;
    let (task, review) = fixture
        .held(fixture.call(
            "delete_rows",
            Some("{\"rows\":\"none\",\"rows\":\"all\",\"n\":99999999999999999999}"),
        ))
        .await;
    let shown: Value = serde_json::from_str(&review.arguments_json).unwrap();
    fixture.answer(&review, ALLOW).await;
    task.await.unwrap().unwrap();
    let sent = fixture.apps.calls.lock().unwrap()[0].1.clone().unwrap();
    assert_eq!(shown, sent);
    assert_eq!(review.arguments_json, sent.to_string());
    assert_eq!(sent["rows"], "all");
}

#[tokio::test]
async fn a_review_past_its_share_of_the_view_is_refused_before_it_is_asked_for() {
    let fixture = Fixture::new().await;
    let large = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_REVIEW_BYTES));
    assert_eq!(
        refused(
            fixture
                .call_tool(fixture.call("delete_rows", Some(&large)))
                .await
        ),
        McpAppError::RequestTooLarge
    );
    // Not asked for: no request on record, no review shown, nothing sent.
    assert_eq!(
        fixture.audit.phases(),
        [McpAppAuditPhase::Refused(
            ConversationErrorCode::McpRequestTooLarge
        )]
    );
    assert!(fixture.app_reviews().await.is_empty());
    // And so a view never loses the app's own tool call to its reviews.
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    assert!(view
        .tools
        .iter()
        .any(|tool| tool.tool_id == fixture.tool_id));
}

#[tokio::test]
async fn an_answer_is_measured_as_the_wire_carries_it() {
    // Under the bound as text, past it once every quote in it is escaped
    // again for the JSON string the wire carries it in.
    let fixture = Fixture::new().await;
    let quoted =
        json!({"content": [{"type": "text", "text": "\"".repeat(MAX_APP_RESULT_BYTES / 3)}]});
    assert!(quoted.to_string().len() < MAX_APP_RESULT_BYTES);
    fixture.apps.answers.lock().unwrap().push(Ok(quoted));
    assert_eq!(
        refused(fixture.call_tool(fixture.call("read_rows", None)).await),
        McpAppError::ResultTooLarge
    );
}

#[tokio::test]
async fn a_session_with_nothing_open_or_too_busy_refuses_before_sending() {
    let fixture = Fixture::new().await;
    for (failure, error, code) in [
        (
            McpAppFailure::NoSession,
            McpAppError::SessionUnavailable,
            ConversationErrorCode::McpSessionUnavailable,
        ),
        (
            McpAppFailure::Busy,
            McpAppError::Busy,
            ConversationErrorCode::TemporarilyUnavailable,
        ),
    ] {
        *fixture.apps.listing_fails.lock().unwrap() = Some(failure);
        assert_eq!(
            refused(fixture.call_tool(fixture.call("read_rows", None)).await),
            error
        );
        assert_eq!(
            fixture.audit.phases().last(),
            Some(&McpAppAuditPhase::Refused(code))
        );
    }
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn a_stopping_gateway_records_every_waiting_call_before_it_returns() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    // Each record takes a while to commit: shutdown must wait for it.
    *fixture.audit.slow.lock().unwrap() = Some(Duration::from_millis(300));
    fixture.service.shutdown().await.unwrap();
    // On record already: shutdown waited for the call's task to record it.
    assert_eq!(
        fixture
            .audit
            .records
            .lock()
            .unwrap()
            .last()
            .map(|record| (record.phase.clone(), record.initiator.clone())),
        Some((
            McpAppAuditPhase::Withdrawn {
                permission_id: review.permission_id.clone(),
                cause: McpAppWithdrawal::ConversationEnded,
            },
            McpAppInitiator::System
        ))
    );
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
}

#[tokio::test]
async fn deleting_the_conversation_withdraws_its_waiting_calls_as_the_deleter() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    // The delete happens; this fixture has no way to erase the provider's own
    // record of the session, which is all it leaves unfinished.
    let deleted = fixture
        .service
        .delete(fixture.id.clone(), caller("delete"))
        .await;
    let stopped = match &deleted {
        Ok(_) => true,
        Err(ConversationError::DeletionIncomplete(failures)) => failures.stop.is_none(),
        Err(_) => false,
    };
    assert!(stopped, "{deleted:?}");
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    let deleter = person_by("delete");
    let last = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert_eq!(
        (last.phase, last.initiator),
        (
            McpAppAuditPhase::Withdrawn {
                permission_id: review.permission_id,
                cause: McpAppWithdrawal::ConversationEnded,
            },
            deleter.clone()
        )
    );
    assert_eq!(
        *fixture.tickets.released_conversations.lock().unwrap(),
        std::slice::from_ref(&deleter)
    );
}

#[tokio::test]
async fn a_persons_delete_names_that_person_on_the_records_it_still_causes() {
    // The agent is not in this run's slots, so the delete itself — not the
    // stop — ends the open review and lets the ticket go. Both name the
    // person on the deletion record.
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    fixture
        .service
        .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
        .await
        .unwrap();
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    fixture
        .service
        .inner
        .conversations
        .lock()
        .await
        .remove(&fixture.id);
    let deleted = fixture
        .service
        .delete(fixture.id.clone(), caller("delete"))
        .await;
    let stopped = match &deleted {
        Ok(_) => true,
        Err(ConversationError::DeletionIncomplete(failures)) => failures.stop.is_none(),
        Err(_) => false,
    };
    assert!(stopped, "{deleted:?}");
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    let deleter = person_by("delete");
    let withdrawals: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| {
            matches!(
                record.phase,
                McpAppAuditPhase::Withdrawn {
                    cause: McpAppWithdrawal::ConversationEnded,
                    ..
                }
            )
        })
        .map(|record| record.initiator.clone())
        .collect();
    assert_eq!(withdrawals.as_slice(), std::slice::from_ref(&deleter));
    assert!(fixture.audit.records.lock().unwrap().iter().any(|record| {
        record.initiator == deleter
            && record.phase
                == McpAppAuditPhase::Withdrawn {
                    permission_id: review.permission_id.clone(),
                    cause: McpAppWithdrawal::ConversationEnded,
                }
    }));
    assert_eq!(
        *fixture.tickets.released_conversations.lock().unwrap(),
        std::slice::from_ref(&deleter)
    );
}

#[tokio::test]
async fn a_release_is_kept_across_a_close_and_before_the_conversation_is_open() {
    let fixture = Fixture::new().await;
    // Closed, then released while nothing of it is open: kept all the same.
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    // A call opens it again, and is refused: the mount stays released.
    assert_eq!(
        refused(fixture.call_tool(fixture.call("delete_rows", None)).await),
        McpAppError::Cancelled
    );
    assert!(fixture.app_reviews().await.is_empty());
    // Another mount of the same tool call is its own.
    let (task, review) = fixture
        .held(McpAppCall {
            app: fixture.app(OTHER_INSTANCE),
            ..fixture.call("delete_rows", None)
        })
        .await;
    fixture.answer(&review, DENY).await;
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalDenied);
}

#[tokio::test]
async fn a_busy_session_is_refused_and_on_record_as_nothing_sent() {
    // The session takes no more requests: the call is refused before it is
    // sent — not recorded as a call made that failed.
    let fixture = Fixture::new().await;
    fixture
        .apps
        .answers
        .lock()
        .unwrap()
        .push(Err(McpAppFailure::Busy));
    assert_eq!(
        refused(fixture.call_tool(fixture.call("read_rows", None)).await),
        McpAppError::Busy
    );
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::Refused(ConversationErrorCode::TemporarilyUnavailable)
        ]
    );
    *fixture.apps.resource.lock().unwrap() = Some(Err(McpAppFailure::Busy));
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
                .await
        ),
        McpAppError::Busy
    );
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused(
            ConversationErrorCode::TemporarilyUnavailable
        ))
    );
}

#[tokio::test]
async fn a_resource_no_answer_could_carry_is_refused_and_never_held() {
    let fixture = Fixture::new().await;
    let sources = |list: &str| -> Vec<String> {
        (0..nessa_sdk::domain::mcp_apps::MAX_CSP_SOURCES)
            .map(|index| format!("https://{list}{index}.{}.example", "x".repeat(400)))
            .collect()
    };
    let csp = UiCsp::new(sources("c"), sources("r"), vec![], vec![]).unwrap();
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p/>".into(),
        csp,
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()));
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(fixture.id.clone(), caller("read"), read(&fixture, INSTANCE))
                .await
        ),
        McpAppError::ResultTooLarge
    );
    assert!(fixture.tickets.issued.lock().unwrap().is_empty());
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Completed(McpAppOutcome::Failed(
            ConversationErrorCode::McpResultTooLarge
        )))
    );
}

#[tokio::test]
async fn a_call_still_running_holds_no_reopening_or_deletion_back() {
    // The call keeps its conversation's apps, never its agent: the history
    // is free to be opened again, or deleted, while it runs.
    let fixture = Fixture::new().await;
    fixture.apps.hold.store(true, Ordering::SeqCst);
    let running = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let call = fixture.call("read_rows", None);
        tokio::spawn(async move { service.call_app_tool(id, caller("held"), call).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.apps.calls() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    // Opened again while the call runs.
    fixture
        .service
        .read(fixture.id.clone(), caller("reopen"))
        .await
        .unwrap();
    // And deleted while it runs: the history is not leased elsewhere.
    let deleted = fixture
        .service
        .delete(fixture.id.clone(), caller("delete"))
        .await;
    // All this fixture leaves unfinished is the provider's own record of the
    // session, which it has no way to erase: the agent stopped, and the
    // history was not leased elsewhere.
    let Err(ConversationError::DeletionIncomplete(failures)) = &deleted else {
        panic!("{deleted:?}")
    };
    let only_the_provider = DeletionFailures {
        provider: failures.provider.clone(),
        ..DeletionFailures::default()
    };
    assert!(failures.provider.is_some(), "{deleted:?}");
    assert_eq!(format!("{failures:?}"), format!("{only_the_provider:?}"));
    fixture.apps.gate.0.add_permits(1);
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_release_during_admission_opens_no_review_and_issues_no_ticket() {
    // The release lands while the destructive call's approval is being
    // recorded, before its review is opened, and while a read's admission is
    // being recorded, before its ticket is issued. Both answer cancelled,
    // and a later call or read of that mount still does.
    let fixture = Fixture::new().await;
    let hold = Hold::default();
    *fixture.audit.hold.lock().unwrap() = Some(hold.clone());
    let calling = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let call = fixture.call("delete_rows", None);
        tokio::spawn(async move { service.call_app_tool(id, caller("call"), call).await })
    };
    tokio::time::timeout(Duration::from_secs(5), hold.waiting.notified())
        .await
        .expect("the call waits in its first record");
    assert!(fixture.app_reviews().await.is_empty());
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert!(fixture.app_reviews().await.is_empty());
    hold.go.add_permits(1);
    assert_eq!(refused(calling.await.unwrap()), McpAppError::Cancelled);
    assert!(fixture.app_reviews().await.is_empty());
    assert_eq!(
        refused(fixture.call_tool(fixture.call("delete_rows", None)).await),
        McpAppError::Cancelled
    );
    let call_records: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|record| (record.phase.clone(), record.initiator.clone()))
        .collect();
    assert!(
        matches!(
            call_records[0].0,
            McpAppAuditPhase::ApprovalRequested { .. }
        ),
        "{call_records:?}"
    );
    assert_eq!(
        call_records[1],
        (
            McpAppAuditPhase::Withdrawn {
                permission_id: match &call_records[0].0 {
                    McpAppAuditPhase::ApprovalRequested { permission_id } => permission_id.clone(),
                    _ => unreachable!(),
                },
                cause: McpAppWithdrawal::AppTornDown,
            },
            McpAppInitiator::System,
        )
    );
    assert_eq!(
        call_records[2],
        (
            McpAppAuditPhase::Refused(ConversationErrorCode::McpCancelled),
            McpAppInitiator::System,
        )
    );

    let reading_fixture = Fixture::new().await;
    *reading_fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    let read_hold = Hold::default();
    *reading_fixture.audit.hold.lock().unwrap() = Some(read_hold.clone());
    let reading = {
        let service = reading_fixture.service.clone();
        let id = reading_fixture.id.clone();
        let read = read(&reading_fixture, INSTANCE);
        tokio::spawn(async move { service.read_app_resource(id, caller("read"), read).await })
    };
    tokio::time::timeout(Duration::from_secs(5), read_hold.waiting.notified())
        .await
        .expect("the read waits in its admission record");
    assert!(reading_fixture.tickets.issued.lock().unwrap().is_empty());
    reading_fixture
        .service
        .release_app(
            reading_fixture.id.clone(),
            caller("release"),
            reading_fixture.app(INSTANCE),
        )
        .await
        .unwrap();
    read_hold.go.add_permits(1);
    assert_eq!(refused(reading.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(reading_fixture.apps.reads.load(Ordering::SeqCst), 0);
    assert!(reading_fixture.tickets.issued.lock().unwrap().is_empty());
    assert_eq!(
        refused(
            reading_fixture
                .service
                .read_app_resource(
                    reading_fixture.id.clone(),
                    caller("read"),
                    read(&reading_fixture, INSTANCE),
                )
                .await
        ),
        McpAppError::Cancelled
    );
    assert!(reading_fixture.tickets.issued.lock().unwrap().is_empty());
    let read_records: Vec<_> = reading_fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .map(|record| (record.phase.clone(), record.initiator.clone()))
        .collect();
    assert_eq!(
        read_records,
        vec![
            (
                McpAppAuditPhase::Admitted,
                McpAppInitiator::App {
                    principal_id: PrincipalId::new("person").unwrap(),
                    surface_id: "panel".into(),
                },
            ),
            (
                McpAppAuditPhase::Refused(ConversationErrorCode::McpCancelled),
                McpAppInitiator::System,
            ),
            (
                McpAppAuditPhase::Refused(ConversationErrorCode::McpCancelled),
                McpAppInitiator::System,
            ),
        ]
    );
}

#[tokio::test]
async fn a_mount_released_after_its_call_was_allowed_is_not_sent() {
    // Allowed, and released while the approval was being recorded: checked
    // once more just before it is handed to the session, it is not.
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    *fixture.audit.slow.lock().unwrap() = Some(Duration::from_millis(300));
    fixture.answer(&review, ALLOW).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(fixture.apps.calls(), 0);
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused(
            ConversationErrorCode::McpCancelled
        ))
    );
}

#[tokio::test]
async fn a_mount_released_after_its_read_was_admitted_is_not_read() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(page("<p/>")));
    // `Admitted` takes a while to record; the release lands meanwhile.
    *fixture.audit.slow.lock().unwrap() = Some(Duration::from_millis(300));
    let reading = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let read = read(&fixture, INSTANCE);
        tokio::spawn(async move { service.read_app_resource(id, caller("read"), read).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(reading.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(fixture.apps.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_mount_released_once_its_call_is_sent_finds_it_sent() {
    // Past its last check the call is sent: a release then ends nothing of
    // it, as it ends nothing of any call already sent, and the call's own
    // end is on record as its own.
    let fixture = Fixture::new().await;
    fixture.apps.hold.store(true, Ordering::SeqCst);
    let running = {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let call = fixture.call("read_rows", None);
        tokio::spawn(async move { service.call_app_tool(id, caller("sent"), call).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.apps.calls() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    fixture.apps.gate.0.add_permits(1);
    running.await.unwrap().unwrap();
    assert!(matches!(
        fixture.audit.phases().last(),
        Some(McpAppAuditPhase::Completed(McpAppOutcome::Answered { .. }))
    ));
}

#[tokio::test]
async fn every_read_of_the_same_app_reviews_has_the_same_revision() {
    // A window polling a conversation sees no change where there is none,
    // and sees one when an app review opens.
    let fixture = Fixture::new().await;
    let (_first, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let read = || fixture.service.read(fixture.id.clone(), caller("read"));
    let once = read().await.unwrap();
    assert_eq!(read().await.unwrap().revision, once.revision);
    let (_second, _) = fixture.held(fixture.call("delete_rows", None)).await;
    assert_ne!(read().await.unwrap().revision, once.revision);
}

#[tokio::test]
async fn a_conversation_deleted_with_no_apps_in_this_run_has_them_deleted() {
    // Never opened in this run, then deleted: its apps are made deleted, so a
    // release or an opening racing the delete finds them so, and cannot
    // build them afresh.
    let fixture = Fixture::new().await;
    let never = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    fixture
        .service
        .close_apps_for_good(&never, &McpAppInitiator::System);
    fixture.drops.settled().await;
    assert!(!fixture
        .audit
        .phases()
        .iter()
        .any(|phase| matches!(phase, McpAppAuditPhase::ContextDropped { .. })));
    let apps = fixture.service.apps_of(&never);
    let opening = apps.begin();
    assert_eq!(
        apps.admit(opening, &fixture.app(INSTANCE)),
        Err(ReviewRefusal::Ended)
    );
}

/// The largest entry a view can hold: a key, and a tool name every
/// character of which takes the most an encoding can make of it.
fn largest_entry(index: usize, origin: ConversationReviewKind) -> ConversationReviewEntry {
    review_entry(&format!("{index:032x}"), origin, &"\u{1}".repeat(1_000))
}

#[test]
fn a_review_entry_is_small_and_of_fixed_size_whatever_its_tool_is_named() {
    for name in [
        "",
        "Write",
        &"\u{1}".repeat(1_000),
        &"é".repeat(1_000),
        &"\"".repeat(99),
    ] {
        let entry = review_entry(&"f".repeat(32), ConversationReviewKind::App, name);
        let tool = serde_json::to_string(&entry.tool_name).unwrap();
        assert!(
            tool.len() - 2 <= MAX_REVIEW_ENTRY_TOOL_NAME_BYTES,
            "{name:?}"
        );
        assert!(name.starts_with(&entry.tool_name));
        let encoded = serde_json::to_vec(&entry).unwrap();
        assert!(encoded.len() < MAX_REVIEW_ENTRY_BYTES, "{name:?}");
    }
}

/// A read's view filled past its bound by a transcript, tool calls and
/// questions, as large as each may be.
async fn overfull(fixture: &Fixture) -> ConversationView {
    let mut view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    view.permissions.clear();
    let message = view.messages[0].clone();
    view.messages = (0..40)
        .map(|_| ConversationMessage {
            authentication_required: None,
            user_text: "y".repeat(8_000),
            ..message.clone()
        })
        .collect();
    view.messages.push(message);
    view.title = Some("t".repeat(512));
    view
}

#[tokio::test]
async fn the_most_reviews_a_view_lists_are_all_listed_at_their_largest_however_full_it_is() {
    // Every review the agent may hold open and every app review, each entry
    // as large as one can be, beside a transcript that overfills the view by
    // itself: the transcript gives way, and not one review is left out.
    let fixture = Fixture::new().await;
    let (_task, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let view = overfull(&fixture).await;
    assert!(serde_json::to_vec(&view).unwrap().len() > MAX_VIEW_BYTES);
    let agent: Vec<_> = (0..MAX_OPEN_REVIEWS)
        .map(|index| largest_entry(index, ConversationReviewKind::Harness))
        .collect();
    let apps: Vec<_> = (MAX_OPEN_REVIEWS..MAX_LISTED_REVIEWS)
        .map(|index| largest_entry(index, ConversationReviewKind::App))
        .collect();
    assert_eq!(apps.len(), MAX_OPEN_APP_REVIEWS);
    let listed = with_reviews(view, agent.clone(), apps.clone());
    assert!(serde_json::to_vec(&listed).unwrap().len() <= MAX_VIEW_BYTES);
    assert_eq!(listed.permissions, [agent, apps].concat());
    assert_eq!(listed.interaction_view_error, None);
    assert!(listed.truncated);
}

#[tokio::test]
async fn reviews_are_listed_whatever_is_confirmed_of_the_transcript() {
    // What a review shows is read from its live owner, never from history,
    // so an unconfirmed history leaves none out.
    let fixture = Fixture::new().await;
    let (_task, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    let entries = view.permissions.clone();
    assert_eq!(entries.len(), 1);
    for state in [
        ConversationTranscriptState::Partial,
        ConversationTranscriptState::Stale,
        ConversationTranscriptState::Unknown,
        ConversationTranscriptState::NotLoaded,
    ] {
        let unconfirmed = ConversationView {
            transcript_state: state,
            ..view.clone()
        };
        let listed = with_reviews(unconfirmed, vec![], entries.clone());
        assert_eq!(listed.permissions, entries, "{state:?}");
    }
}

#[tokio::test]
async fn the_revision_changes_exactly_with_which_reviews_are_listed() {
    let fixture = Fixture::new().await;
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    let entry = |index: usize| largest_entry(index, ConversationReviewKind::App);
    let none = with_reviews(view.clone(), vec![], vec![]);
    let two = with_reviews(view.clone(), vec![entry(1)], vec![entry(2)]);
    assert_eq!(
        with_reviews(view.clone(), vec![entry(1)], vec![entry(2)]).revision,
        two.revision
    );
    assert_ne!(two.revision, none.revision);
    assert_ne!(
        with_reviews(view.clone(), vec![entry(1)], vec![]).revision,
        two.revision
    );
    assert_ne!(
        with_reviews(view.clone(), vec![entry(1)], vec![entry(3)]).revision,
        two.revision
    );
    assert!(two.revision.len() <= view.revision.len().max(16));
}

#[tokio::test]
async fn every_open_app_review_is_listed_and_answerable_from_its_live_owner() {
    // As many app reviews as may be open, each read by its key from the
    // store that holds it, and answered.
    let fixture = Fixture::new().await;
    let mut tasks = Vec::new();
    let room = MAX_APP_REVIEW_BYTES / MAX_OPEN_APP_REVIEWS - 600;
    for _ in 0..MAX_OPEN_APP_REVIEWS {
        let arguments = format!("{{\"a\":\"{}\"}}", "x".repeat(room));
        let (task, _) = fixture
            .held(fixture.call("delete_rows", Some(&arguments)))
            .await;
        tasks.push(task);
    }
    let reviews = fixture.app_reviews().await;
    assert_eq!(reviews.len(), MAX_OPEN_APP_REVIEWS);
    for review in &reviews {
        assert!(review.arguments_json.len() > room);
        fixture.answer(review, ALLOW).await;
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert!(fixture.app_reviews().await.is_empty());
}
