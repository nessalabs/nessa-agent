//! An MCP App's calls through the conversation service (#348): one test per
//! row of the "MCP App call" state table — each policy refusal, a call sent
//! at once, a destructive call allowed, denied, cancelled, expired and
//! withdrawn (by its request going, its mount being released, and its
//! conversation ending), a server's failures, a resource held behind a
//! ticket — and the audit each leaves.
use super::*;
use crate::app_call_test_support::{caller, Fixture, INSTANCE, OTHER_INSTANCE, SERVER, URI};
use crate::conversation::application::app_reviews::{ALLOW, DENY, MAX_OPEN_APP_REVIEWS};
use crate::conversation::application::mcp_apps::McpAppFailure;
use crate::conversation::application::view::ConversationPermissionOrigin;
use crate::mcp_servers::domain::MAX_APP_ARGUMENTS_BYTES;
use nessa_auth::domain::PrincipalId;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::domain::mcp_apps::{ToolHints, UiCsp, UiResource, UiVisibility};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
        Some(&McpAppAuditPhase::Refused("invalid_request"))
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
        )
        .await;
    assert!(matches!(
        again,
        Err(ConversationError::Agent(AgentError::StalePermission))
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
    assert_eq!(
        fixture.tickets.released_apps.lock().unwrap().clone(),
        [fixture.app(INSTANCE)]
    );
    // The other mount still waits, and is answered as it is answered.
    let left = fixture.app_reviews().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].permission_id, other_review.permission_id);
    fixture.answer(&other_review, ALLOW).await;
    other.await.unwrap().unwrap();
    assert!(fixture.audit.phases().iter().any(|phase| matches!(
        phase,
        McpAppAuditPhase::Withdrawn {
            cause: McpAppWithdrawal::AppTornDown,
            ..
        }
    )));
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
    assert_eq!(
        fixture
            .tickets
            .released_conversations
            .load(Ordering::SeqCst),
        1
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
    assert_eq!(
        withdrawals,
        [McpAppInitiator::System, McpAppInitiator::System]
    );
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
    assert_eq!(resource.sha256, hex(&Sha256::digest(b"<p>chart</p>")));
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
            ticket_digest: hex(&Sha256::digest(resource.ticket.as_bytes())),
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
            "temporarily_unavailable"
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
