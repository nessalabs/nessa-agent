//! Durable evidence of where each attempt at a submission came from,
//! committed before the submission is admitted.
//!
//! ```text
//!   submit ──record──▶ this adapter ──▶ <root>/turn-origins/<name>.json
//!      │                                          │
//!      └── refuses the submission on failure      └── one file per attempt,
//!                                                     private, named from a
//!                                                     digest of the identity
//! ```
//!
//! One record per conversation, submission and attempt, so a retried attempt
//! reconciles with the record it left, and one attempt that came back
//! claiming another origin fails closed. An attempt whose origin could not
//! be read is recorded apart (`-unread`), so reading it on a retry adds a
//! record rather than contradicting one. The name is derived, never copied
//! from input: the submission and request identities are whatever a caller
//! sent.
use super::evidence_record::{commit_evidence, digest_of};
use crate::application::{
    ConversationError, ConversationFuture, ConversationTurnOriginAudit,
    ConversationTurnOriginAuditRecord, TurnOrigin,
};
use nessa_local_storage::create_directory;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Most bytes one stored record may be before it is treated as damaged: two
/// 256-byte identities, a 256-byte grant, an app's server and tool names, and
/// the envelope fit well inside it.
const MAX_RECORD_BYTES: usize = 16_384;

pub struct DurableConversationTurnOriginAudit {
    directory: PathBuf,
}

impl DurableConversationTurnOriginAudit {
    pub fn new(directory: PathBuf) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory })
    }
}

impl ConversationTurnOriginAudit for DurableConversationTurnOriginAudit {
    fn record(&self, record: ConversationTurnOriginAuditRecord) -> ConversationFuture<'_, ()> {
        // An origin that could not be read is weaker evidence, not another
        // claim: kept apart, so a retry that reads it is no contradiction.
        let id = format!(
            "turn-origin-{}-{}-{}{}",
            record.conversation_id,
            digest_of(record.execution_id.as_bytes()),
            digest_of(record.request_id.as_bytes()),
            if record.origin.is_none() {
                "-unread"
            } else {
                ""
            }
        );
        let value = record_value(&id, &record);
        commit_evidence(self.directory.clone(), id, value, MAX_RECORD_BYTES)
    }
}

/// The record as it is stored.
fn record_value(id: &str, record: &ConversationTurnOriginAuditRecord) -> Value {
    json!({
        "recordId": id,
        "kind": "conversation_turn_origin",
        "target": {
            "conversationId": record.conversation_id.to_string(),
            "organizationId": record.organization_id.as_str(),
            "executionId": record.execution_id,
            "requestId": record.request_id,
        },
        "origin": record.origin.as_ref().map_or_else(
            || json!({"kind": "unread"}),
            |origin| origin_value(&record.conversation_id.to_string(), origin),
        ),
        "sentFrom": record.sent_from.as_ref().map(|sent| json!({
            "client": sent.client,
            "spoken": sent.spoken,
        })),
        "initiator": {
            "principalId": record.initiator_principal_id.as_str(),
            "surfaceId": record.initiator_surface_id,
        },
        "observedAtMs": record.observed_at_ms,
    })
}

/// An origin as the record says it, with the names [`TurnOrigin::kind`] and
/// its device's [`name`](crate::application::Device::name) give it.
fn origin_value(conversation: &str, origin: &TurnOrigin) -> Value {
    let mut value = json!({"kind": origin.kind()});
    match origin {
        TurnOrigin::Person { device } => value["device"] = json!(device.name()),
        TurnOrigin::Grant { grant, device } => {
            value["grant"] = json!(grant);
            value["device"] = json!(device.name());
        }
        TurnOrigin::Agent { thread, execution } => {
            value["from"] = json!({"conversationId": thread, "executionId": execution});
        }
        TurnOrigin::Resumed { execution } | TurnOrigin::Continued { execution } => {
            value["from"] = json!({"conversationId": conversation, "executionId": execution});
        }
        TurnOrigin::Event { source } => value["source"] = json!(source),
        TurnOrigin::App { server, tool } => {
            value["app"] = json!({"server": server, "tool": tool});
        }
        TurnOrigin::AutoPick | TurnOrigin::Internal => {}
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{Device, SentFrom};
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use nessa_protocol::conversation::domain::ConversationId;

    fn record(origin: Option<TurnOrigin>, request: &str) -> ConversationTurnOriginAuditRecord {
        ConversationTurnOriginAuditRecord {
            conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            organization_id: OrganizationId::new("organization").unwrap(),
            execution_id: "turn/../1".into(),
            request_id: request.into(),
            origin,
            sent_from: Some(SentFrom {
                client: "nessa-browser".into(),
                spoken: true,
            }),
            initiator_principal_id: PrincipalId::new("owner").unwrap(),
            initiator_surface_id: "credential-1".into(),
            observed_at_ms: 5,
        }
    }

    fn stored(directory: &std::path::Path) -> Vec<Value> {
        let mut names: Vec<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        names.sort();
        names
            .iter()
            .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn each_attempt_is_recorded_once_with_its_origin_and_client() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("turn-origins");
        let audit = DurableConversationTurnOriginAudit::new(directory.clone()).unwrap();
        let first = record(
            Some(TurnOrigin::Person {
                device: Device::Computer,
            }),
            "request-1",
        );
        audit.record(first.clone()).await.unwrap();
        // The same attempt again, seen later: it agrees.
        let mut again = first.clone();
        again.observed_at_ms = 9;
        audit.record(again).await.unwrap();
        // Another attempt at the same submission is its own record.
        audit
            .record(record(Some(TurnOrigin::AutoPick), "request-2"))
            .await
            .unwrap();
        // An attempt whose origin could not be read, then read on a retry
        // (or the other way round), is not a contradiction.
        audit.record(record(None, "request-1")).await.unwrap();
        audit.record(record(None, "request-2")).await.unwrap();
        audit.record(record(None, "request-3")).await.unwrap();
        audit
            .record(record(Some(TurnOrigin::AutoPick), "request-3"))
            .await
            .unwrap();
        let records = stored(&directory);
        assert_eq!(records.len(), 6);
        let person = records
            .iter()
            .find(|record| record["origin"]["kind"] == "person")
            .unwrap();
        assert_eq!(person["kind"], "conversation_turn_origin");
        assert_eq!(person["origin"]["device"], "computer");
        assert_eq!(person["sentFrom"]["client"], "nessa-browser");
        assert_eq!(person["sentFrom"]["spoken"], true);
        assert_eq!(person["target"]["executionId"], "turn/../1");
        assert_eq!(person["target"]["requestId"], "request-1");
        assert_eq!(person["initiator"]["surfaceId"], "credential-1");
        assert_eq!(person["observedAtMs"], 5);
        assert!(records
            .iter()
            .any(|record| record["origin"] == json!({"kind": "autoPick"})));
        // The caller's identities never name a file.
        for entry in std::fs::read_dir(&directory).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains(".."), "{name}");
        }
    }

    #[tokio::test]
    async fn one_attempt_claiming_another_origin_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurableConversationTurnOriginAudit::new(root.path().join("o")).unwrap();
        audit
            .record(record(
                Some(TurnOrigin::Person {
                    device: Device::Phone,
                }),
                "request-1",
            ))
            .await
            .unwrap();
        assert!(matches!(
            audit
                .record(record(
                    Some(TurnOrigin::Event {
                        source: "github".into()
                    }),
                    "request-1"
                ))
                .await,
            Err(ConversationError::Audit)
        ));
    }

    #[test]
    fn every_origin_names_where_it_came_from() {
        let conversation = "00000000-0000-4000-8000-000000000001";
        let cases = [
            (
                TurnOrigin::Grant {
                    grant: "grant-1".into(),
                    device: Device::PairedDevice,
                },
                json!({"kind": "grant", "grant": "grant-1", "device": "pairedDevice"}),
            ),
            (
                TurnOrigin::Agent {
                    thread: "thread-a".into(),
                    execution: "7".into(),
                },
                json!({"kind": "agent", "from": {"conversationId": "thread-a", "executionId": "7"}}),
            ),
            (
                TurnOrigin::Resumed {
                    execution: "5".into(),
                },
                json!({"kind": "resumed", "from": {"conversationId": conversation, "executionId": "5"}}),
            ),
            (
                TurnOrigin::Continued {
                    execution: "4".into(),
                },
                json!({"kind": "continued", "from": {"conversationId": conversation, "executionId": "4"}}),
            ),
            (
                TurnOrigin::Event {
                    source: "hook".into(),
                },
                json!({"kind": "event", "source": "hook"}),
            ),
            (
                TurnOrigin::App {
                    server: "s".into(),
                    tool: "t".into(),
                },
                json!({"kind": "app", "app": {"server": "s", "tool": "t"}}),
            ),
            (TurnOrigin::Internal, json!({"kind": "internal"})),
        ];
        for (origin, expected) in cases {
            assert_eq!(origin_value(conversation, &origin), expected);
        }
        let unread = record_value("id", &record(None, "r"));
        assert_eq!(unread["origin"], json!({"kind": "unread"}));
    }
}
