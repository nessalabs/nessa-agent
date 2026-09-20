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
        record_value(&AttachmentAuditRecord::HoldCreated { hold: hold() }),
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
    // Permission to upload is itself recorded, with when it runs out.
    assert_eq!(
        record_value(&AttachmentAuditRecord::TicketIssued { ticket: ticket() }),
        json!({
            "kind": "attachment_ticket_issued",
            "target": target(false),
            "transition": {"before": "absent", "after": "ticket_outstanding"},
            "cause": "upload_requested",
            "initiator": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
            "expiresAtMs": 301_000,
        })
    );
    assert_eq!(
        record_value(&AttachmentAuditRecord::TicketReplaced { ticket: ticket() }),
        json!({
            "kind": "attachment_ticket_replaced",
            "target": target(false),
            "transition": {"before": "ticket_outstanding", "after": "ticket_replaced"},
            "cause": "upload_requested_again",
            "initiator": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
            "expiresAtMs": 301_000,
        })
    );
    // The file was already kept: the target pairs what this ticket uploaded
    // with what the conversation holds, and nothing changed.
    let mut already = target(true);
    already["uploaded"] = target(false)["uploaded"].clone();
    assert_eq!(
        record_value(&AttachmentAuditRecord::AlreadyHeld {
            ticket: ticket(),
            hold: hold(),
        }),
        json!({
            "kind": "attachment_already_held",
            "target": already,
            "transition": {"before": "held", "after": "held"},
            "cause": "uploaded",
            "initiator": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
            "heldSinceMs": 2_000,
        })
    );
    // Nobody asked for a hold to be taken back, and the record does not say so.
    assert_eq!(
        record_value(&AttachmentAuditRecord::HoldReverted {
            hold: hold(),
            cause: RevertCause::AuditUnconfirmed,
        }),
        json!({
            "kind": "attachment_hold_reverted",
            "target": target(true),
            "transition": {"before": "pending", "after": "absent"},
            "cause": "audit_unconfirmed",
            "initiator": {"kind": "automatic"},
            "uploadedBy": uploader,
            "correlationId": "begin-1",
            "requestedAtMs": 1_000,
        })
    );
    for (cause, name) in [
        (RevertCause::ConfirmationFailed, "confirmation_failed"),
        (RevertCause::RemovedBeforeUsable, "removed_before_usable"),
    ] {
        assert_eq!(
            record_value(&AttachmentAuditRecord::HoldReverted {
                hold: hold(),
                cause,
            })["cause"],
            name
        );
    }
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
            "expiresAtMs": 301_000,
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
            "expiresAtMs": 301_000,
        })
    );
    // Withdrawn by the closer, from the caller it had been issued to.
    assert_eq!(
        record_value(&AttachmentAuditRecord::TicketWithdrawn {
            ticket: ticket(),
            release: release(),
        }),
        json!({
            "kind": "attachment_ticket_withdrawn",
            "target": target(false),
            "transition": {"before": "ticket_outstanding", "after": "ticket_withdrawn"},
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
            was: HoldState::Held,
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
    // A hold released before its upload finished recording it says so.
    assert_eq!(
        record_value(&AttachmentAuditRecord::HoldReleased {
            hold: hold(),
            was: HoldState::Pending,
            release: release(),
        })["transition"],
        json!({"before": "pending", "after": "absent"})
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
fn every_refusal_has_its_own_name_and_it_is_the_routes_name_for_it() {
    // One word per fact: what the upload route answers is what the trail says.
    for (reason, wire) in [
        (UploadRejection::SizeMismatch, "size_mismatch"),
        (UploadRejection::DigestMismatch, "digest_mismatch"),
        (UploadRejection::UploadInterrupted, "upload_interrupted"),
        (UploadRejection::UploadTimeout, "upload_timeout"),
        (UploadRejection::StorageUnavailable, "storage_unavailable"),
        (
            UploadRejection::ImageInputUnsupported,
            "image_input_unsupported",
        ),
        (UploadRejection::UnsupportedImage, "unsupported_image"),
        (UploadRejection::ImageTooLarge, "image_too_large"),
    ] {
        assert_eq!(rejection(reason), wire);
    }
    let names: Vec<_> = [
        UploadRejection::SizeMismatch,
        UploadRejection::DigestMismatch,
        UploadRejection::UploadInterrupted,
        UploadRejection::UploadTimeout,
        UploadRejection::StorageUnavailable,
        UploadRejection::ImageInputUnsupported,
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
