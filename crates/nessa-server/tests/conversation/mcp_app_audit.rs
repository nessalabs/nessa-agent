//! What the durable MCP App record holds, what it keeps out, and what it
//! refuses to overwrite.
use super::*;
use crate::conversation::application::McpAppRef;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock that answers what it is told, one tick per question.
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

fn audit_at(directory: &Path, now: u64) -> DurableMcpAppAudit {
    DurableMcpAppAudit::new(
        directory.to_path_buf(),
        Arc::new(TestClock(AtomicU64::new(now))),
    )
    .unwrap()
}

fn app_initiator() -> McpAppInitiator {
    McpAppInitiator::App {
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "panel".into(),
    }
}

fn person_initiator() -> McpAppInitiator {
    McpAppInitiator::Person {
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "panel".into(),
        request_id: "answer-7".into(),
    }
}

fn call(phase: McpAppAuditPhase, initiator: McpAppInitiator) -> McpAppAuditRecord {
    McpAppAuditRecord {
        conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000348").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        call_id: "call-1".into(),
        request_id: "app-request-1".into(),
        app: McpAppRef {
            execution_id: "execution-1".into(),
            tool_id: "tool-call-1".into(),
            instance_id: "mount-1".into(),
        },
        ask: McpAppAsk::CallTool {
            server: "weather".into(),
            tool: "forecast".into(),
        },
        initiator,
        phase,
    }
}

fn records(directory: &Path) -> Vec<Value> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect()
}

fn sole_record(directory: &Path) -> Value {
    let mut all = records(directory);
    assert_eq!(all.len(), 1, "{all:?}");
    all.pop().unwrap()
}

#[tokio::test]
async fn an_app_call_records_its_target_ask_initiator_and_request() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 500);

    audit
        .record(call(McpAppAuditPhase::Admitted, app_initiator()))
        .await
        .unwrap();

    let stored = sole_record(&directory);
    assert_eq!(stored["kind"], "mcp_app_call");
    assert_eq!(
        stored["target"],
        json!({
            "conversationId": "00000000-0000-4000-8000-000000000348",
            "organizationId": "org",
            "app": {"executionId": "execution-1", "toolId": "tool-call-1", "instanceId": "mount-1"},
            "ask": {"kind": "call_tool", "server": "weather", "tool": "forecast"},
        })
    );
    assert_eq!(stored["callId"], "call-1");
    assert_eq!(stored["requestId"], "app-request-1");
    assert_eq!(stored["phase"], json!({"kind": "admitted"}));
    assert_eq!(
        stored["initiator"],
        json!({"kind": "app", "onBehalfOf": {"principalId": "owner", "surfaceId": "panel"}})
    );
    assert_eq!(stored["observedAtMs"], 500);
}

#[tokio::test]
async fn a_resource_read_records_the_uri_it_asked_for() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 500);
    let mut record = call(McpAppAuditPhase::Refused("not_an_app"), app_initiator());
    record.ask = McpAppAsk::ReadResource {
        server: "weather".into(),
        uri: "ui://weather/map".into(),
    };

    audit.record(record).await.unwrap();

    let stored = sole_record(&directory);
    assert_eq!(
        stored["target"]["ask"],
        json!({"kind": "read_resource", "server": "weather", "uri": "ui://weather/map"})
    );
    assert_eq!(
        stored["phase"],
        json!({"kind": "refused", "code": "not_an_app"})
    );
}

/// Every phase variant, each with the initiator that takes that step, and the
/// fields and cause it carries. Also what keeps each phase's `kind` distinct:
/// one request, eleven phases, eleven records.
#[tokio::test]
async fn every_phase_of_one_request_is_its_own_record() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1_000);
    let review = || "permission-1".to_string();
    let cases = [
        (
            McpAppAuditPhase::Refused("forbidden"),
            app_initiator(),
            json!({"kind": "refused", "code": "forbidden"}),
        ),
        (
            McpAppAuditPhase::Admitted,
            app_initiator(),
            json!({"kind": "admitted"}),
        ),
        (
            McpAppAuditPhase::ApprovalRequested {
                permission_id: review(),
            },
            app_initiator(),
            json!({"kind": "approval_requested", "permissionId": "permission-1"}),
        ),
        (
            McpAppAuditPhase::Approved {
                permission_id: review(),
            },
            person_initiator(),
            json!({"kind": "approved", "permissionId": "permission-1"}),
        ),
        (
            McpAppAuditPhase::Denied {
                permission_id: review(),
            },
            person_initiator(),
            json!({"kind": "denied", "permissionId": "permission-1"}),
        ),
        (
            McpAppAuditPhase::Expired {
                permission_id: review(),
            },
            McpAppInitiator::System,
            json!({"kind": "expired", "permissionId": "permission-1"}),
        ),
        (
            McpAppAuditPhase::Withdrawn {
                permission_id: review(),
                cause: McpAppWithdrawal::AppTornDown,
            },
            McpAppInitiator::System,
            json!({"kind": "withdrawn", "permissionId": "permission-1", "cause": "app_torn_down"}),
        ),
        (
            McpAppAuditPhase::Completed(McpAppOutcome::Answered {
                is_error: true,
                bytes: 42,
            }),
            McpAppInitiator::System,
            json!({"kind": "completed", "outcome": {"kind": "answered", "isError": true, "bytes": 42}}),
        ),
        (
            McpAppAuditPhase::TicketIssued {
                ticket_digest: "ticket-digest".into(),
                size: 9,
                sha256: "bytes-digest".into(),
            },
            app_initiator(),
            json!({"kind": "ticket_issued", "ticketDigest": "ticket-digest", "size": 9, "sha256": "bytes-digest"}),
        ),
        (
            McpAppAuditPhase::TicketRedeemed {
                ticket_digest: "ticket-digest".into(),
            },
            app_initiator(),
            json!({"kind": "ticket_redeemed", "ticketDigest": "ticket-digest"}),
        ),
        (
            McpAppAuditPhase::TicketExpired {
                ticket_digest: "ticket-digest".into(),
            },
            McpAppInitiator::System,
            json!({"kind": "ticket_expired", "ticketDigest": "ticket-digest"}),
        ),
    ];
    let count = cases.len();
    for (phase, initiator, _) in cases.clone() {
        audit.record(call(phase, initiator)).await.unwrap();
    }

    let stored = records(&directory);
    assert_eq!(stored.len(), count);
    for (_, initiator, phase) in cases {
        let record = stored
            .iter()
            .find(|record| record["phase"] == phase)
            .unwrap_or_else(|| panic!("no record of {phase}"));
        let expected = match initiator {
            McpAppInitiator::App { .. } => {
                json!({"kind": "app", "onBehalfOf": {"principalId": "owner", "surfaceId": "panel"}})
            }
            McpAppInitiator::Person { .. } => json!({
                "kind": "person", "principalId": "owner", "surfaceId": "panel",
                "requestId": "answer-7",
            }),
            McpAppInitiator::System => json!({"kind": "system"}),
        };
        assert_eq!(record["initiator"], expected, "{phase}");
        assert_eq!(record["requestId"], "app-request-1");
        assert_eq!(record["target"]["app"]["toolId"], "tool-call-1");
        assert_eq!(record["target"]["app"]["instanceId"], "mount-1");
    }
}

#[tokio::test]
async fn each_withdrawal_and_failure_keeps_its_cause() {
    for (cause, name) in [
        (McpAppWithdrawal::RequestCancelled, "request_cancelled"),
        (McpAppWithdrawal::AppTornDown, "app_torn_down"),
        (McpAppWithdrawal::ConversationEnded, "conversation_ended"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("mcp-apps");
        audit_at(&directory, 1)
            .record(call(
                McpAppAuditPhase::Withdrawn {
                    permission_id: "permission-1".into(),
                    cause,
                },
                McpAppInitiator::System,
            ))
            .await
            .unwrap();
        assert_eq!(sole_record(&directory)["phase"]["cause"], name);
    }

    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    audit_at(&directory, 1)
        .record(call(
            McpAppAuditPhase::Completed(McpAppOutcome::Failed("timed_out")),
            McpAppInitiator::System,
        ))
        .await
        .unwrap();
    assert_eq!(
        sole_record(&directory)["phase"]["outcome"],
        json!({"kind": "failed", "code": "timed_out"})
    );
}

#[tokio::test]
async fn a_retried_step_is_one_record_and_keeps_its_first_observation() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 700);

    audit
        .record(call(McpAppAuditPhase::Admitted, app_initiator()))
        .await
        .unwrap();
    audit
        .record(call(McpAppAuditPhase::Admitted, app_initiator()))
        .await
        .unwrap();

    let stored = sole_record(&directory);
    assert_eq!(stored["observedAtMs"], 700);
}

#[tokio::test]
async fn another_call_app_mount_or_conversation_is_its_own_record() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1);
    audit
        .record(call(McpAppAuditPhase::Admitted, app_initiator()))
        .await
        .unwrap();

    // Another request, another app in the same execution, another mount of the
    // same app, another conversation: each is its own step, not a retry.
    let mut other_request = call(McpAppAuditPhase::Admitted, app_initiator());
    other_request.call_id = "call-2".into();
    let mut other_app = call(McpAppAuditPhase::Admitted, app_initiator());
    other_app.app.tool_id = "tool-call-2".into();
    let mut other_mount = call(McpAppAuditPhase::Admitted, app_initiator());
    other_mount.app.instance_id = "mount-2".into();
    let mut other_conversation = call(McpAppAuditPhase::Admitted, app_initiator());
    other_conversation.conversation_id =
        ConversationId::new("00000000-0000-4000-8000-000000000349").unwrap();
    for record in [other_request, other_app, other_mount, other_conversation] {
        audit.record(record).await.unwrap();
    }
    assert_eq!(records(&directory).len(), 5);
}

#[tokio::test]
async fn contradictory_evidence_for_the_same_step_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1);
    let approved = || {
        call(
            McpAppAuditPhase::Approved {
                permission_id: "permission-1".into(),
            },
            person_initiator(),
        )
    };
    audit.record(approved()).await.unwrap();
    let before = sole_record(&directory);

    let mut other_permission = approved();
    other_permission.phase = McpAppAuditPhase::Approved {
        permission_id: "permission-2".into(),
    };
    let mut other_ask = approved();
    other_ask.ask = McpAppAsk::CallTool {
        server: "weather".into(),
        tool: "delete_everything".into(),
    };
    let mut other_person = approved();
    other_person.initiator = McpAppInitiator::Person {
        principal_id: PrincipalId::new("someone-else").unwrap(),
        surface_id: "panel".into(),
        request_id: "answer-7".into(),
    };
    let mut system = approved();
    system.initiator = McpAppInitiator::System;
    let mut other_organization = approved();
    other_organization.organization_id = OrganizationId::new("other-org").unwrap();
    for contradiction in [
        other_permission,
        other_ask,
        other_person,
        system,
        other_organization,
    ] {
        assert!(matches!(
            audit.record(contradiction).await,
            Err(ConversationError::Audit)
        ));
    }
    // The first evidence stands, unrepaired.
    assert_eq!(sole_record(&directory), before);
}

#[tokio::test]
async fn a_write_that_fails_is_a_visible_audit_failure() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1);
    std::fs::remove_dir_all(&directory).unwrap();

    assert!(matches!(
        audit
            .record(call(McpAppAuditPhase::Admitted, app_initiator()))
            .await,
        Err(ConversationError::Audit)
    ));
}

#[tokio::test]
async fn a_record_larger_than_any_stored_is_refused_rather_than_written() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1);
    let mut record = call(McpAppAuditPhase::Refused("too_large"), app_initiator());
    record.ask = McpAppAsk::ReadResource {
        server: "weather".into(),
        uri: "x".repeat(MAX_RECORD_BYTES),
    };

    assert!(matches!(
        audit.record(record).await,
        Err(ConversationError::Audit)
    ));
    assert!(records(&directory).is_empty());
}

#[test]
fn stored_damage_is_refused_rather_than_read_as_agreement() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path();
    let value = json!({"recordId": "r", "phase": {"kind": "admitted"}, "observedAtMs": 1});
    let put = |path: &Path, bytes: &[u8]| {
        open(path, OpenMode::CreateNew)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    };

    assert!(matches!(
        stored_evidence(&directory.join("missing.json"), &value, "r"),
        Ok(Stored::Absent)
    ));
    let junk = directory.join("junk.json");
    put(&junk, b"not json");
    assert!(matches!(
        stored_evidence(&junk, &value, "r"),
        Err(ConversationError::Audit)
    ));
    let huge = directory.join("huge.json");
    put(&huge, &vec![b' '; MAX_RECORD_BYTES + 1]);
    assert!(matches!(
        stored_evidence(&huge, &value, "r"),
        Err(ConversationError::Audit)
    ));
}

/// The record is evidence of who asked for what and what happened, not a copy
/// of the call. The port carries no arguments, result or bytes, so this holds
/// the record to the closed set of fields the adapter writes: a field added to
/// the record has to be added here, deliberately.
#[tokio::test]
async fn no_argument_result_or_resource_content_is_stored() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("mcp-apps");
    let audit = audit_at(&directory, 1);
    audit
        .record(call(
            McpAppAuditPhase::Completed(McpAppOutcome::Answered {
                is_error: false,
                bytes: 1_024,
            }),
            McpAppInitiator::System,
        ))
        .await
        .unwrap();
    audit
        .record(call(
            McpAppAuditPhase::TicketIssued {
                ticket_digest: "ticket-digest".into(),
                size: 9,
                sha256: "bytes-digest".into(),
            },
            app_initiator(),
        ))
        .await
        .unwrap();

    fn keys(value: &Value, prefix: &str, into: &mut Vec<String>) {
        if let Value::Object(object) = value {
            for (key, child) in object {
                let path = format!("{prefix}{key}");
                into.push(path.clone());
                keys(child, &format!("{path}."), into);
            }
        }
    }
    let allowed = [
        "recordId",
        "kind",
        "target",
        "target.conversationId",
        "target.organizationId",
        "target.app",
        "target.app.executionId",
        "target.app.toolId",
        "target.app.instanceId",
        "target.ask",
        "target.ask.kind",
        "target.ask.server",
        "target.ask.tool",
        "callId",
        "requestId",
        "phase",
        "phase.kind",
        "phase.outcome",
        "phase.outcome.kind",
        "phase.outcome.isError",
        "phase.outcome.bytes",
        "phase.ticketDigest",
        "phase.size",
        "phase.sha256",
        "initiator",
        "initiator.kind",
        "initiator.onBehalfOf",
        "initiator.onBehalfOf.principalId",
        "initiator.onBehalfOf.surfaceId",
        "observedAtMs",
    ];
    for record in records(&directory) {
        let mut found = Vec::new();
        keys(&record, "", &mut found);
        for key in found {
            assert!(allowed.contains(&key.as_str()), "unexpected field {key}");
        }
        let text = record.to_string();
        for forbidden in [
            "arguments",
            "result",
            "content",
            "structuredContent",
            "ticket\"",
        ] {
            assert!(!text.contains(forbidden), "{forbidden} in {text}");
        }
    }
}
