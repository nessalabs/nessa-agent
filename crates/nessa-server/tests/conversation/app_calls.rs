//! An MCP App's calls through the conversation service (#348): one test per
//! row of the "MCP App call" state table — each policy refusal, a call sent
//! at once, a destructive call allowed, denied, cancelled, expired and
//! withdrawn (by its request going, its mount being released, and its
//! conversation ending), a server's failures, a resource held behind a
//! ticket — and the audit each leaves.
use super::*;
use crate::app_call_test_support::{caller, Fixture, INSTANCE, OTHER_INSTANCE, SERVER, URI};
use crate::conversation::application::app_reviews::{
    ALLOW, DENY, MAX_APP_REVIEW_BYTES, MAX_OPEN_APP_REVIEWS,
};
use crate::conversation::application::mcp_apps::TicketEnd;
use crate::conversation::application::projection::MAX_VIEW_BYTES;
use crate::conversation::application::view::{ConversationMessage, ConversationPermissionOrigin};
use crate::conversation::application::DeletionFailures;
use crate::mcp_servers::domain::MAX_APP_ARGUMENTS_BYTES;
use nessa_auth::domain::PrincipalId;
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::domain::mcp_apps::{ToolHints, UiCsp, UiResource, UiVisibility};
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
        Some(&McpAppAuditPhase::Refused(McpAppCode::InvalidRequest))
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
            McpAppCode::TemporarilyUnavailable
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
        McpAppAuditPhase::Refused(McpAppCode::Cancelled),
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
            McpAppAuditPhase::Completed(McpAppOutcome::Failed(McpAppCode::Cancelled)),
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
            McpAppAuditPhase::Refused(McpAppCode::ToolNotForApp)
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
        [McpAppAuditPhase::Refused(McpAppCode::RequestTooLarge)]
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
            McpAppCode::SessionUnavailable,
        ),
        (
            McpAppFailure::Busy,
            McpAppError::Busy,
            McpAppCode::TemporarilyUnavailable,
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
            McpAppAuditPhase::Refused(McpAppCode::TemporarilyUnavailable)
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
            McpAppCode::TemporarilyUnavailable
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
            McpAppCode::ResultTooLarge
        )))
    );
}

#[tokio::test]
async fn an_app_review_is_shown_only_beside_a_confirmed_transcript() {
    // The client refuses a view of unconfirmed history that offers any
    // control: there, the review waits unseen rather than lose the view.
    let fixture = Fixture::new().await;
    let (_task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    let mut view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    view.permissions.clear();
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
        let shown = with_app_reviews(unconfirmed.clone(), vec![review.clone()]);
        assert!(shown.permissions.is_empty(), "{state:?}");
        assert_eq!(shown.revision, unconfirmed.revision);
    }
    let shown = with_app_reviews(view, vec![review.clone()]);
    assert_eq!(
        shown
            .permissions
            .iter()
            .map(|shown| shown.permission_id.as_str())
            .collect::<Vec<_>>(),
        [review.permission_id.as_str()]
    );
}

#[tokio::test]
async fn app_reviews_are_the_last_of_a_full_view_to_go() {
    // A transcript that fills the view by itself: the reviews still show,
    // and so does the tool call whose app asked — the transcript gives way.
    let fixture = Fixture::new().await;
    let (_task, review) = fixture
        .held(fixture.call(
            "delete_rows",
            Some(&format!("{{\"a\":\"{}\"}}", "x".repeat(12_000))),
        ))
        .await;
    let mut view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    view.permissions.clear();
    let message = view.messages[0].clone();
    view.messages = (0..40)
        .map(|_| ConversationMessage {
            user_text: "y".repeat(2_000),
            ..message.clone()
        })
        .collect();
    view.messages.push(message);
    // As the read hands it out: bounded, with its reviews, and no further.
    let shown = with_app_reviews(view, vec![review.clone()]);
    assert!(serde_json::to_vec(&shown).unwrap().len() <= MAX_VIEW_BYTES);
    assert_eq!(
        shown
            .permissions
            .iter()
            .map(|shown| shown.permission_id.as_str())
            .collect::<Vec<_>>(),
        [review.permission_id.as_str()]
    );
    assert!(shown
        .tools
        .iter()
        .any(|tool| tool.tool_id == fixture.tool_id));
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
async fn a_mount_released_after_its_call_was_allowed_is_not_sent() {
    // Allowed, and released while the approval was being recorded: checked
    // once more as it is sent, it is not.
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
        Some(&McpAppAuditPhase::Refused(McpAppCode::Cancelled))
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
async fn an_app_review_never_pushes_the_agents_own_reviews_out_of_view() {
    // The agent's reviews fill the view: an app's review is the one that
    // waits unseen, and the view says so — an app's server cannot hide what
    // the agent is asking.
    let fixture = Fixture::new().await;
    let (_task, review) = fixture
        .held(fixture.call(
            "delete_rows",
            Some(&format!("{{\"a\":\"{}\"}}", "x".repeat(12_000))),
        ))
        .await;
    let mut view = fixture
        .service
        .read(fixture.id.clone(), caller("read"))
        .await
        .unwrap();
    let agents: Vec<_> = (0..4)
        .map(|index| ConversationPermission {
            permission_id: format!("agent-{index}"),
            arguments_json: format!("{{\"a\":\"{}\"}}", "y".repeat(13_000)),
            origin: ConversationPermissionOrigin::Harness,
            ..review.clone()
        })
        .collect();
    view.permissions = agents.clone();
    view.interaction_view_error = None;
    // As the read had it before its app reviews were added.
    view.revision = view.revision.split(":app:").next().unwrap().to_owned();
    let unshown = view.revision.clone();
    let shown = with_app_reviews(view, vec![review.clone()]);
    assert!(serde_json::to_vec(&shown).unwrap().len() <= MAX_VIEW_BYTES);
    assert_eq!(
        shown
            .permissions
            .iter()
            .map(|shown| shown.permission_id.clone())
            .collect::<Vec<_>>(),
        agents
            .iter()
            .map(|agent| agent.permission_id.clone())
            .collect::<Vec<_>>()
    );
    assert!(shown.interaction_view_error.is_some());
    // Not shown, so not in the revision either.
    assert_eq!(shown.revision, unshown);
}
