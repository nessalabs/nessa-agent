//! What each committed record says, and that it is committed privately.
use super::*;
use crate::attachments::domain::TicketLifetime;
use crate::attachments_test_support::{
    attachment, conversation, digest_of, organization, principal, ManualClock, CONVERSATION,
};
use std::fs;

fn ticket() -> UploadTicket {
    UploadTicket::new(
        organization("org"),
        conversation(CONVERSATION),
        attachment(b"sent", "image/heic"),
        Caller::new(principal("owner"), "panel", "begin-1").unwrap(),
        TicketLifetime::starting(1_000).unwrap(),
    )
}
fn hold() -> Hold {
    Hold::from_upload(&ticket(), attachment(b"kept", "image/jpeg"), 2_000).unwrap()
}
fn release() -> ReleaseEvidence {
    ReleaseEvidence {
        cause: ReleaseCause::ConversationClosed,
        caller: Caller::new(principal("closer"), "phone", "close-1").unwrap(),
        requested_at_ms: 3_000,
    }
}
fn target(stored: bool) -> Value {
    json!({
        "organizationId": "org",
        "conversationId": CONVERSATION,
        "uploaded": {
            "digest": digest_of(b"sent").to_string(),
            "mediaType": "image/heic",
            "size": 4,
        },
        "stored": if stored {
            json!({
                "digest": digest_of(b"kept").to_string(),
                "mediaType": "image/jpeg",
                "size": 4,
            })
        } else {
            Value::Null
        },
    })
}

#[test]
fn every_record_names_its_target_transition_cause_initiator_and_request() {
    let uploader = json!({"kind": "caller", "principalId": "owner", "surfaceId": "panel"});
    let closer = json!({"kind": "caller", "principalId": "closer", "surfaceId": "phone"});
    assert_eq!(
        record_value(&AttachmentAuditRecord::HoldCreated {
            hold: hold(),
            before: HoldState::Absent,
        }),
        json!({
            "kind": "attachment_hold_created",
            "target": target(true),
            "transition": {"before": "absent", "after": "held"},
            "cause": "uploaded",
            "initiator": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
            "uploadedAtMs": 2_000,
        })
    );
    assert_eq!(
        record_value(&AttachmentAuditRecord::HoldCreated {
            hold: hold(),
            before: HoldState::Held,
        })["transition"],
        json!({"before": "held", "after": "held"})
    );
    assert_eq!(
        record_value(&AttachmentAuditRecord::UploadRejected {
            ticket: ticket(),
            reason: UploadRejection::DigestMismatch,
        }),
        json!({
            "kind": "attachment_upload_rejected",
            "target": target(false),
            "transition": {"before": "ticket_outstanding", "after": "ticket_used_without_hold"},
            "cause": "digest_mismatch",
            "initiator": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
        })
    );
    // Nobody expires a ticket, and the record does not pretend somebody did.
    assert_eq!(
        record_value(&AttachmentAuditRecord::TicketExpired { ticket: ticket() }),
        json!({
            "kind": "attachment_ticket_expired",
            "target": target(false),
            "transition": {"before": "ticket_outstanding", "after": "ticket_expired"},
            "cause": "ticket_lifetime_elapsed",
            "initiator": {"kind": "automatic"},
            "issuedTo": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
            "expiredAtMs": 301_000,
        })
    );
    // Withdrawn by the closer, from the caller it had been issued to.
    assert_eq!(
        record_value(&AttachmentAuditRecord::TicketVoided {
            ticket: ticket(),
            release: release(),
        }),
        json!({
            "kind": "attachment_ticket_voided",
            "target": target(false),
            "transition": {"before": "ticket_outstanding", "after": "ticket_voided"},
            "cause": "conversation_closed",
            "initiator": closer,
            "issuedTo": uploader,
            "correlationId": "close-1",
            "ticketCorrelationId": "begin-1",
            "requestedAtMs": 3_000,
        })
    );
    // The closer released it; whoever uploaded it did not.
    assert_eq!(
        record_value(&AttachmentAuditRecord::HoldReleased {
            hold: hold(),
            release: release(),
        }),
        json!({
            "kind": "attachment_hold_released",
            "target": target(true),
            "transition": {"before": "held", "after": "absent"},
            "cause": "conversation_closed",
            "initiator": closer,
            "correlationId": "close-1",
            "requestedAtMs": 3_000,
        })
    );
    assert_eq!(
        record_value(&AttachmentAuditRecord::BlobRemoved {
            hold: hold(),
            release: release(),
        }),
        json!({
            "kind": "attachment_bytes_removed",
            "target": target(true),
            "transition": {"before": "stored", "after": "absent"},
            "cause": "last_hold_released",
            "releaseCause": "conversation_closed",
            "initiator": closer,
            "correlationId": "close-1",
            "requestedAtMs": 3_000,
        })
    );
}

#[test]
fn every_refusal_has_its_own_name() {
    let names: Vec<_> = [
        UploadRejection::SizeMismatch,
        UploadRejection::DigestMismatch,
        UploadRejection::BodyInterrupted,
        UploadRejection::DeadlineElapsed,
        UploadRejection::StorageUnavailable,
        UploadRejection::UnsupportedImage,
        UploadRejection::ImageTooLarge,
        UploadRejection::NormalizationFailed,
    ]
    .into_iter()
    .map(rejection)
    .collect();
    let distinct: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(distinct.len(), names.len());
}

#[tokio::test]
async fn a_record_is_committed_privately_with_its_own_identity_and_observation_time() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("audit");
    let audit =
        DurableAttachmentAudit::new(directory.clone(), Arc::new(ManualClock::at(9_000))).unwrap();
    for _ in 0..2 {
        audit
            .record(AttachmentAuditRecord::TicketExpired { ticket: ticket() })
            .await
            .unwrap();
    }
    let mut identities = Vec::new();
    for entry in fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["kind"], "attachment_ticket_expired");
        // Observed now; requested when the ticket was issued. Neither is
        // presented as the other.
        assert_eq!(value["observedAtMs"], 9_000);
        assert_eq!(value["requestedAtMs"], 1_000);
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("{}.json", value["recordId"].as_str().unwrap())
        );
        identities.push(value["recordId"].as_str().unwrap().to_owned());
    }
    assert_eq!(identities.len(), 2);
    assert_ne!(identities[0], identities[1]);
}

#[cfg(unix)]
#[tokio::test]
async fn an_audit_directory_that_is_not_private_or_not_writable_is_a_visible_failure() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let shared = root.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(DurableAttachmentAudit::new(shared, Arc::new(ManualClock::at(0))).is_err());

    let directory = root.path().join("audit");
    let audit =
        DurableAttachmentAudit::new(directory.clone(), Arc::new(ManualClock::at(0))).unwrap();
    fs::remove_dir(&directory).unwrap();
    assert_eq!(
        audit
            .record(AttachmentAuditRecord::TicketExpired { ticket: ticket() })
            .await,
        Err(AuditUnavailable)
    );
}
