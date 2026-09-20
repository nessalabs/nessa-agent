//! The words `attachment.begin` answers in.
use super::*;
use crate::attachments::application::TicketSecret;
use crate::attachments_test_support::{attachment, digest_of};

#[test]
fn a_stored_answer_carries_no_ticket_and_a_ticket_carries_no_reference() {
    let stored = begin_result(
        "begin-1".into(),
        BeginOutcome::Stored(attachment(b"kept", "image/jpeg")),
    );
    assert_eq!(stored.state, "stored");
    assert_eq!(
        (stored.ticket, stored.expires_at_ms),
        (None, None),
        "a stored file needs no upload"
    );
    assert_eq!(stored.digest, Some(digest_of(b"kept").to_string()));
    assert_eq!(stored.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(stored.size, Some(4));

    let required = begin_result(
        "begin-1".into(),
        BeginOutcome::UploadRequired {
            ticket: TicketSecret::from_bytes([0xab; 32]),
            expires_at_ms: 9,
        },
    );
    assert_eq!(required.state, "upload_required");
    assert_eq!(required.ticket, Some("ab".repeat(32)));
    assert_eq!(required.expires_at_ms, Some(9));
    assert_eq!(
        (required.digest, required.mime_type, required.size),
        (None, None, None),
        "nothing is stored yet, so there is nothing to refer to"
    );
    assert_eq!(required.request_id, "begin-1");
}

#[test]
fn every_refusal_has_its_own_code() {
    let codes = [
        (BeginError::InvalidRequest, "invalid_request"),
        (BeginError::ConversationNotFound, "conversation_not_found"),
        (BeginError::ImagesUnsupported, "image_input_unsupported"),
        (BeginError::Capacity, "attachment_capacity"),
        (BeginError::Storage, "attachment_storage_unavailable"),
        (BeginError::Audit, "audit_unavailable"),
        (BeginError::Unavailable, "temporarily_unavailable"),
    ];
    // The words, not the variants: what a client reads is the string the
    // generated code spells, and this is the one place that is asserted.
    for (error, code) in codes {
        assert_eq!(error_code(error).as_str(), code);
    }
}

#[test]
fn a_ticket_secret_never_formats_itself() {
    let secret = TicketSecret::from_bytes([0xab; 32]);
    assert_eq!(format!("{secret:?}"), "TicketSecret(..)");
    assert_eq!(TicketSecret::parse(&secret.expose()), Some(secret));
}
