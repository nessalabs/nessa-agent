//! The attachment service over doubles: tickets, uploads, normalization,
//! release, and what reaches the audit port when each of them fails.
use super::*;
use crate::attachments::domain::Caller;
use crate::attachments::domain::{HoldState, TicketLimits, TICKET_LIFETIME_MS};
use crate::attachments_test_support::{
    attachment, begin_request, caller, conversation, digest_of, organization, principal,
    ChannelBody, CountingSecrets, FixedOwnership, Fixture, ManualClock, MemoryStore,
    RecordingAudit, StubNormalizer, CONVERSATION, NOW_MS, OTHER_CONVERSATION,
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore};

const PDF: &str = "application/pdf";
const BYTES: &[u8] = b"twenty bytes of file";

/// Bounded only in total, for tests that are not about the narrower bounds.
fn tickets(total: usize) -> TicketLimits {
    TicketLimits {
        total,
        per_organization: total,
        per_conversation: total,
    }
}
fn fixture() -> Fixture {
    Fixture::new(AttachmentLimits::default())
}
fn release_request(conversation_id: &str) -> ReleaseRequest {
    ReleaseRequest {
        organization_id: organization("org"),
        conversation_id: conversation(conversation_id),
        cause: ReleaseCause::ConversationClosed,
        principal_id: principal("closer"),
        surface_id: "phone".into(),
        correlation_id: "close-1".into(),
    }
}
fn rejected(reason: UploadRejection) -> UploadError {
    UploadError::Rejected {
        reason,
        evidence: AuditDelivery::Recorded,
    }
}
/// Nothing was kept, nothing is still staged, and the one record says why.
fn assert_refused(fixture: &Fixture, reason: UploadRejection) {
    assert_eq!(fixture.store.blob_count(), 0);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::UploadRejected {
        ticket,
        reason: recorded,
    }] = records.as_slice()
    else {
        panic!("one refusal expected, got {records:?}")
    };
    assert_eq!(*recorded, reason);
    assert_eq!(ticket.attachment(), &attachment(BYTES, PDF));
    assert_eq!(ticket.conversation_id(), &conversation(CONVERSATION));
    assert_eq!(ticket.caller().principal_id(), &principal("owner"));
    assert_eq!(ticket.caller().surface_id(), "panel");
    assert_eq!(ticket.caller().action_id(), "begin-1");
    assert_eq!(ticket.lifetime().issued_at_ms(), NOW_MS);
}

#[tokio::test]
async fn an_upload_becomes_a_hold_attributed_to_the_ticket_and_begin_then_needs_no_upload() {
    let fixture = fixture();
    let outcome = fixture
        .service
        .begin(
            caller("org", "owner"),
            begin_request(CONVERSATION, BYTES, PDF),
        )
        .await
        .unwrap();
    let BeginOutcome::UploadRequired {
        ticket,
        expires_at_ms,
    } = outcome
    else {
        panic!("first begin must ask for the bytes")
    };
    assert_eq!(expires_at_ms, NOW_MS + TICKET_LIFETIME_MS);
    assert_eq!(ticket.expose().len(), 64);
    assert!(fixture.audit.taken().is_empty());

    fixture.clock.set(NOW_MS + 5);
    let stored = fixture
        .service
        .receive(&ticket.expose(), None, ChannelBody::of(BYTES, 3))
        .await
        .unwrap();
    // Not an image, so it is kept exactly as it arrived.
    assert_eq!(stored, attachment(BYTES, PDF));
    assert_eq!(fixture.store.blob(digest_of(BYTES)).unwrap(), BYTES);
    assert!(fixture.normalizer.seen.lock().unwrap().is_empty());
    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::HoldCreated { hold }] = records.as_slice() else {
        panic!("one hold expected, got {records:?}")
    };
    assert_eq!(hold.uploaded(), &stored);
    assert_eq!(hold.stored(), &stored);
    assert_eq!(hold.organization_id(), &organization("org"));
    assert_eq!(hold.conversation_id(), &conversation(CONVERSATION));
    assert_eq!(hold.uploaded_by().principal_id(), &principal("owner"));
    assert_eq!(hold.uploaded_by().action_id(), "begin-1");
    assert_eq!(hold.ticket_issued_at_ms(), NOW_MS);
    assert_eq!(hold.uploaded_at_ms(), NOW_MS + 5);

    // The same file again needs nothing; a different description of it does.
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await
            .unwrap(),
        BeginOutcome::Stored(stored.clone())
    );
    for (conversation_id, media_type) in [(CONVERSATION, "text/plain"), (OTHER_CONVERSATION, PDF)] {
        assert!(matches!(
            fixture
                .service
                .begin(
                    caller("org", "owner"),
                    begin_request(conversation_id, BYTES, media_type)
                )
                .await
                .unwrap(),
            BeginOutcome::UploadRequired { .. }
        ));
    }
    assert!(fixture.audit.taken().is_empty());
}

#[tokio::test]
async fn a_hold_answers_only_inside_the_conversation_and_organization_it_was_uploaded_into() {
    let fixture = fixture();
    let stored = fixture.upload(CONVERSATION, BYTES, PDF).await;
    let holds = |organization_id: &'static str, conversation_id: &'static str| {
        let (service, stored) = (fixture.service.clone(), stored.clone());
        async move {
            service
                .holds(
                    &organization(organization_id),
                    &conversation(conversation_id),
                    &stored,
                )
                .await
                .unwrap()
        }
    };
    assert!(holds("org", CONVERSATION).await);
    assert!(!holds("org", OTHER_CONVERSATION).await);
    assert!(!holds("other-org", CONVERSATION).await);
    assert!(!holds("Org", CONVERSATION).await);
    for wrong in [attachment(BYTES, "text/plain"), attachment(b"other", PDF)] {
        assert!(!fixture
            .service
            .holds(&organization("org"), &conversation(CONVERSATION), &wrong)
            .await
            .unwrap());
    }
}

#[tokio::test]
async fn beginning_in_a_conversation_the_caller_does_not_own_is_not_found_and_issues_nothing() {
    let fixture = fixture();
    fixture
        .ownership
        .give("00000000-0000-4000-8000-0000000000b1", "other-org", "owner");
    for (who, conversation_id) in [
        (caller("org", "intruder"), CONVERSATION),
        (caller("other-org", "owner"), CONVERSATION),
        (
            caller("org", "owner"),
            "00000000-0000-4000-8000-0000000000b1",
        ),
        (
            caller("org", "owner"),
            "00000000-0000-4000-8000-0000000000ff",
        ),
    ] {
        assert_eq!(
            fixture
                .service
                .begin(who, begin_request(conversation_id, BYTES, PDF))
                .await,
            Err(BeginError::ConversationNotFound)
        );
    }
    fixture.ownership.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::Unavailable)
    );
    // No ticket was made for any of them: the first secret is still unissued.
    fixture.ownership.unavailable.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture.ticket(CONVERSATION, BYTES, PDF).await,
        "01".repeat(32)
    );
}

#[tokio::test]
async fn a_request_that_does_not_describe_a_file_is_refused_before_anyone_is_asked() {
    let fixture = fixture();
    let valid = || begin_request(CONVERSATION, BYTES, PDF);
    let mut requests = Vec::new();
    let changes: [fn(&mut BeginUpload); 9] = [
        |request: &mut BeginUpload| request.conversation_id = "not-a-uuid".into(),
        |request: &mut BeginUpload| {
            request.conversation_id = "00000000-0000-4000-8000-0000000000A1".into()
        },
        |request: &mut BeginUpload| request.digest = request.digest.to_uppercase(),
        |request: &mut BeginUpload| request.digest = "sha256:00".into(),
        |request: &mut BeginUpload| request.media_type = "application/pdf; q=1".into(),
        |request: &mut BeginUpload| request.size = 0,
        |request: &mut BeginUpload| request.size = 64 * 1024 * 1024 + 1,
        |request: &mut BeginUpload| request.request_id = " ".into(),
        |request: &mut BeginUpload| request.request_id = "a".repeat(257),
    ];
    for change in changes {
        let mut request = valid();
        change(&mut request);
        requests.push(request);
    }
    for request in requests {
        assert_eq!(
            fixture.service.begin(caller("org", "owner"), request).await,
            Err(BeginError::InvalidRequest)
        );
    }
    assert_eq!(fixture.ownership.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_ticket_works_once_even_when_its_upload_failed() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture
        .service
        .receive(&ticket, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );

    // A failed upload spends the ticket just the same: the caller begins again.
    let failed = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    assert_eq!(
        fixture
            .service
            .receive(&failed, None, ChannelBody::of(b"wrong", 64))
            .await,
        Err(rejected(UploadRejection::SizeMismatch))
    );
    assert_eq!(
        fixture
            .service
            .receive(&failed, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
    for malformed in ["", "zz", &"0".repeat(63), &"A".repeat(64), &"0".repeat(65)] {
        assert_eq!(
            fixture
                .service
                .receive(malformed, None, ChannelBody::of(BYTES, 64))
                .await,
            Err(UploadError::TicketInvalid)
        );
    }
    assert_eq!(fixture.store.held().len(), 1);
}

#[tokio::test]
async fn an_expired_ticket_is_refused_and_recorded_as_nobodys_doing() {
    let fixture = fixture();
    let presented = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let forgotten = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;

    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    assert_eq!(
        fixture
            .service
            .receive(&presented, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketExpired {
            evidence: AuditDelivery::Recorded
        })
    );
    // The next begin accounts for the one nobody came back for.
    assert!(matches!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, b"next", PDF)
            )
            .await
            .unwrap(),
        BeginOutcome::UploadRequired { .. }
    ));
    let records = fixture.audit.taken();
    let conversations: Vec<_> = records
        .iter()
        .map(|record| match record {
            AttachmentAuditRecord::TicketExpired { ticket } => {
                assert_eq!(
                    ticket.lifetime().expires_at_ms(),
                    NOW_MS + TICKET_LIFETIME_MS
                );
                ticket.conversation_id().to_string()
            }
            other => panic!("only expiry expected, got {other:?}"),
        })
        .collect();
    assert_eq!(conversations, [CONVERSATION, OTHER_CONVERSATION]);
    assert_eq!(
        fixture
            .service
            .receive(&forgotten, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
    assert!(fixture.store.held().is_empty());
}

#[tokio::test]
async fn an_expiry_that_cannot_be_recorded_fails_the_begin_but_the_tickets_are_still_gone() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(1),
        ..AttachmentLimits::default()
    });
    let stale = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::Audit)
    );
    fixture.audit.refusing.store(false, Ordering::SeqCst);
    // The expired ticket no longer occupies the book's only place, or works.
    let fresh = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    assert!(fresh != stale);
    assert_eq!(
        fixture
            .service
            .receive(&stale, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
    assert!(fixture.audit.taken().is_empty());
    fixture
        .service
        .receive(&fresh, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();

    // Presented rather than swept, the refusal itself says its evidence was lost.
    let late = fixture.ticket(OTHER_CONVERSATION, b"late", PDF).await;
    fixture.clock.set(NOW_MS + 3 * TICKET_LIFETIME_MS);
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&late, None, ChannelBody::of(b"late", 64))
            .await,
        Err(UploadError::TicketExpired {
            evidence: AuditDelivery::Unavailable
        })
    );
}

#[tokio::test]
async fn outstanding_tickets_are_bounded_across_every_caller() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(2),
        ..AttachmentLimits::default()
    });
    let first = fixture.ticket(CONVERSATION, b"one", PDF).await;
    fixture.ticket(OTHER_CONVERSATION, b"two", PDF).await;
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, b"three", PDF)
            )
            .await,
        Err(BeginError::Capacity)
    );
    // Using a ticket frees its place.
    fixture
        .service
        .receive(&first, None, ChannelBody::of(b"one", 64))
        .await
        .unwrap();
    fixture.ticket(CONVERSATION, b"three", PDF).await;
}

#[tokio::test]
async fn bytes_that_are_not_the_described_file_are_never_kept() {
    // Too short, right length but other bytes, and a declared length that
    // already disagrees before a byte is read.
    for (sent, declared, reason) in [
        (&BYTES[..19], None, UploadRejection::SizeMismatch),
        (
            b"twenty bytes of FILE".as_slice(),
            None,
            UploadRejection::DigestMismatch,
        ),
        (BYTES, Some(21), UploadRejection::SizeMismatch),
        (BYTES, Some(0), UploadRejection::SizeMismatch),
    ] {
        let fixture = fixture();
        let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
        assert_eq!(
            fixture
                .service
                .receive(&ticket, declared, ChannelBody::of(sent, 4))
                .await,
            Err(rejected(reason)),
            "{declared:?}"
        );
        assert_refused(&fixture, reason);
    }
}

#[tokio::test]
async fn a_body_that_runs_long_is_abandoned_at_the_first_byte_too_many() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    // Twenty bytes were promised. The sender stays open with far more behind
    // it: only reading on would reach the end, and the service must not.
    for _ in 0..3 {
        sender.send(Ok(vec![b'x'; 8])).unwrap();
    }
    for _ in 0..1000 {
        sender.send(Ok(vec![b'x'; 8192])).unwrap();
    }
    assert_eq!(
        fixture.service.receive(&ticket, None, body).await,
        Err(rejected(UploadRejection::SizeMismatch))
    );
    assert_eq!(fixture.store.written.load(Ordering::SeqCst), 16);
    drop(sender);
    assert_refused(&fixture, UploadRejection::SizeMismatch);
}

#[tokio::test]
async fn an_interrupted_body_spends_the_ticket_and_leaves_nothing_behind() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    sender.send(Err(UploadInterrupted)).unwrap();
    assert_eq!(
        fixture.service.receive(&ticket, None, body).await,
        Err(rejected(UploadRejection::UploadInterrupted))
    );
    assert_refused(&fixture, UploadRejection::UploadInterrupted);
}

#[tokio::test]
async fn a_caller_that_goes_away_mid_upload_does_not_take_the_evidence_with_it() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    // Issuing the ticket was itself recorded; wait for the record after that one.
    fixture.audit.recorded.notified().await;
    let service = fixture.service.clone();
    let presented = ticket.clone();
    let request = tokio::spawn(async move { service.receive(&presented, None, body).await });
    // The handler is dropped while the transfer waits for more bytes...
    while fixture.store.written.load(Ordering::SeqCst) != 10 {
        tokio::task::yield_now().await;
    }
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    // ...and then the connection it was reading from closes.
    sender.send(Err(UploadInterrupted)).unwrap();
    fixture.audit.recorded.notified().await;
    assert_refused(&fixture, UploadRejection::UploadInterrupted);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
}

#[tokio::test(start_paused = true)]
async fn a_stalled_body_is_given_up_at_the_deadline() {
    let fixture = Fixture::new(AttachmentLimits {
        upload_deadline: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    let service = fixture.service.clone();
    let upload = tokio::spawn(async move { service.receive(&ticket, None, body).await });
    while fixture.store.written.load(Ordering::SeqCst) != 10 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!upload.is_finished());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        upload.await.unwrap(),
        Err(rejected(UploadRejection::UploadTimeout))
    );
    assert_refused(&fixture, UploadRejection::UploadTimeout);
    drop(sender);
}

#[tokio::test]
async fn uploads_in_progress_are_bounded_and_a_refused_one_keeps_its_ticket() {
    let fixture = Fixture::new(AttachmentLimits {
        max_uploads: 1,
        ..AttachmentLimits::default()
    });
    let slow = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let waiting = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    let service = fixture.service.clone();
    let upload = tokio::spawn(async move { service.receive(&slow, None, body).await });
    while fixture.store.written.load(Ordering::SeqCst) != 10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        fixture
            .service
            .receive(&waiting, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::Busy)
    );
    sender.send(Ok(BYTES[10..].to_vec())).unwrap();
    drop(sender);
    upload.await.unwrap().unwrap();
    // Busy refused the request, not the ticket.
    fixture
        .service
        .receive(&waiting, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();
}

#[tokio::test]
async fn storage_that_fails_is_a_refusal_with_evidence_not_a_hold() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.store.keep_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(rejected(UploadRejection::StorageUnavailable))
    );
    assert_refused(&fixture, UploadRejection::StorageUnavailable);

    fixture.store.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::Storage)
    );
    fixture.store.unavailable.store(false, Ordering::SeqCst);
    fixture.secrets.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::Unavailable)
    );
}

#[tokio::test]
async fn a_hold_that_cannot_be_recorded_is_taken_back() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::AuditUnavailable { reverted: true })
    );
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
    assert!(!fixture
        .service
        .holds(
            &organization("org"),
            &conversation(CONVERSATION),
            &attachment(BYTES, PDF)
        )
        .await
        .unwrap());

    // Bytes another conversation already holds survive the taking back.
    fixture.audit.refusing.store(false, Ordering::SeqCst);
    fixture.upload(OTHER_CONVERSATION, BYTES, PDF).await;
    let again = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    fixture.store.discard_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&again, None, ChannelBody::of(BYTES, 64))
            .await,
        // Honest about the worse case too: unrecorded and not taken back.
        Err(UploadError::AuditUnavailable { reverted: false })
    );
    // What is left behind is pending, which nothing that asks can see.
    assert_eq!(fixture.store.pending(), 1);
    assert!(!fixture
        .service
        .holds(
            &organization("org"),
            &conversation(CONVERSATION),
            &attachment(BYTES, PDF)
        )
        .await
        .unwrap());
    assert_eq!(fixture.store.blob(digest_of(BYTES)).unwrap(), BYTES);
}

#[tokio::test]
async fn a_refusal_that_cannot_be_recorded_is_still_the_refusal_it_was() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(b"short", 64))
            .await,
        Err(UploadError::Rejected {
            reason: UploadRejection::SizeMismatch,
            evidence: AuditDelivery::Unavailable
        })
    );
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_image_is_held_as_its_normalized_self_and_recognized_by_what_was_uploaded() {
    let original = b"a very large heic photograph";
    let normalized = b"small jpeg";
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(normalized, "image/jpeg"),
    );
    let stored = fixture.upload(CONVERSATION, original, "image/heic").await;

    assert_eq!(stored, attachment(normalized, "image/jpeg"));
    assert_eq!(
        *fixture.normalizer.seen.lock().unwrap(),
        [(original.to_vec(), "image/heic".to_owned())]
    );
    // Only the normalized bytes are kept, under their own digest.
    assert_eq!(fixture.store.blob(stored.digest()).unwrap(), normalized);
    assert!(fixture.store.blob(digest_of(original)).is_none());
    assert_eq!(fixture.store.blob_count(), 1);
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);

    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::HoldCreated { hold, .. }] = records.as_slice() else {
        panic!("one hold expected, got {records:?}")
    };
    assert_eq!(hold.uploaded(), &attachment(original, "image/heic"));
    assert_eq!(hold.stored(), &stored);

    let (organization_id, conversation_id) = (organization("org"), conversation(CONVERSATION));
    assert!(fixture
        .service
        .holds(&organization_id, &conversation_id, &stored)
        .await
        .unwrap());
    // A message cannot refer to the file that was sent, only the one kept.
    assert!(!fixture
        .service
        .holds(
            &organization_id,
            &conversation_id,
            &attachment(original, "image/heic")
        )
        .await
        .unwrap());
    // Beginning the same upload again answers with what to refer to.
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, original, "image/heic")
            )
            .await
            .unwrap(),
        BeginOutcome::Stored(stored.clone())
    );
    // The stored file was never uploaded, so describing it starts an upload.
    assert!(matches!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, normalized, "image/jpeg")
            )
            .await
            .unwrap(),
        BeginOutcome::UploadRequired { .. }
    ));
}

#[tokio::test]
async fn an_image_that_cannot_be_normalized_is_refused_for_its_own_reason() {
    for (answer, reason) in [
        (
            StubNormalizer::failing(NormalizeError::Unsupported),
            UploadRejection::UnsupportedImage,
        ),
        (
            StubNormalizer::failing(NormalizeError::TooLarge),
            UploadRejection::ImageTooLarge,
        ),
        (
            StubNormalizer::failing(NormalizeError::Failed),
            UploadRejection::NormalizationFailed,
        ),
        // A normalizer is outside code: what it hands back is checked too.
        (
            StubNormalizer::producing(b"x", "IMAGE/PNG"),
            UploadRejection::NormalizationFailed,
        ),
        (
            StubNormalizer::producing(b"", "image/png"),
            UploadRejection::NormalizationFailed,
        ),
        (
            StubNormalizer::producing(b"%PDF", "application/pdf"),
            UploadRejection::NormalizationFailed,
        ),
    ] {
        let fixture = Fixture::with_normalizer(AttachmentLimits::default(), answer);
        let ticket = fixture.ticket(CONVERSATION, BYTES, "image/png").await;
        assert_eq!(
            fixture
                .service
                .receive(&ticket, None, ChannelBody::of(BYTES, 64))
                .await,
            Err(rejected(reason))
        );
        assert_eq!(fixture.store.blob_count(), 0);
        assert!(fixture.store.held().is_empty());
        assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
        assert!(matches!(
            fixture.audit.taken().as_slice(),
            [AttachmentAuditRecord::UploadRejected { reason: recorded, .. }] if *recorded == reason
        ));
    }
}

#[tokio::test]
async fn an_image_is_never_shown_to_the_normalizer_before_it_proved_to_be_the_described_file() {
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(b"small", "image/png"),
    );
    let ticket = fixture.ticket(CONVERSATION, BYTES, "image/png").await;
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(b"twenty bytes of FILE", 64))
            .await,
        Err(rejected(UploadRejection::DigestMismatch))
    );
    assert!(fixture.normalizer.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bytes_are_stored_once_and_go_with_their_last_hold() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.upload(OTHER_CONVERSATION, BYTES, PDF).await;
    fixture.upload(OTHER_CONVERSATION, b"only here", PDF).await;
    assert_eq!(fixture.store.blob_count(), 2);
    assert_eq!(fixture.store.held().len(), 3);
    fixture.audit.taken();

    fixture.clock.set(NOW_MS + 9);
    fixture
        .service
        .release(release_request(OTHER_CONVERSATION))
        .await
        .unwrap();
    assert_eq!(fixture.store.blob(digest_of(BYTES)).unwrap(), BYTES);
    assert!(fixture.store.blob(digest_of(b"only here")).is_none());
    let records = fixture.audit.taken();
    let mut released = 0;
    let mut removed = Vec::new();
    for record in &records {
        let (hold, release) = match record {
            AttachmentAuditRecord::HoldReleased { hold, was, release } => {
                assert_eq!(*was, HoldState::Held);
                released += 1;
                (hold, release)
            }
            AttachmentAuditRecord::BlobRemoved { hold, release } => {
                removed.push(hold.stored().digest());
                (hold, release)
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(hold.conversation_id(), &conversation(OTHER_CONVERSATION));
        // The closer is the initiator, not whoever uploaded.
        assert_eq!(hold.uploaded_by().principal_id(), &principal("owner"));
        assert_eq!(release.caller.principal_id(), &principal("closer"));
        assert_eq!(release.caller.surface_id(), "phone");
        assert_eq!(release.caller.action_id(), "close-1");
        assert_eq!(release.cause, ReleaseCause::ConversationClosed);
        assert_eq!(release.requested_at_ms, NOW_MS + 9);
    }
    assert_eq!(released, 2);
    assert_eq!(removed, [digest_of(b"only here")]);

    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    assert_eq!(fixture.store.blob_count(), 0);
    assert_eq!(fixture.audit.taken().len(), 2);
    // Releasing nothing is not a failure and records nothing.
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    assert!(fixture.audit.taken().is_empty());
}

#[tokio::test]
async fn a_release_withdraws_the_conversations_unused_tickets_in_the_closers_name() {
    let fixture = fixture();
    let withdrawn = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let untouched = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    fixture.clock.set(NOW_MS + 7);
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();

    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::TicketWithdrawn { ticket, release }] = records.as_slice() else {
        panic!("one withdrawal expected, got {records:?}")
    };
    assert_eq!(ticket.conversation_id(), &conversation(CONVERSATION));
    assert_eq!(ticket.caller().principal_id(), &principal("owner"));
    assert_eq!(release.caller.principal_id(), &principal("closer"));
    assert_eq!(release.caller.action_id(), "close-1");
    assert_eq!(release.requested_at_ms, NOW_MS + 7);
    // Nothing can arrive in the closed conversation; the other is unaffected.
    assert_eq!(
        fixture
            .service
            .receive(&withdrawn, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
    fixture
        .service
        .receive(&untouched, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();
    assert_eq!(fixture.store.held().len(), 1);

    // Withdrawn even when the store cannot be asked, and the evidence still kept.
    let stranded = fixture.ticket(CONVERSATION, b"again", PDF).await;
    fixture.audit.taken();
    fixture.store.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 1,
            audit_failures: 0
        })
    );
    assert!(matches!(
        fixture.audit.taken().as_slice(),
        [AttachmentAuditRecord::TicketWithdrawn { .. }]
    ));
    fixture.store.unavailable.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&stranded, None, ChannelBody::of(b"again", 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
}

#[tokio::test]
async fn one_hold_that_will_not_release_does_not_stop_the_others() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"stuck", PDF).await;
    fixture.upload(CONVERSATION, b"third", PDF).await;
    fixture.store.stick(digest_of(b"stuck"));
    fixture.audit.taken();

    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 1,
            audit_failures: 0
        })
    );
    let held = fixture.store.held();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].stored().digest(), digest_of(b"stuck"));
    assert_eq!(fixture.store.blob_count(), 1);
    // Both that did release are on record, bytes and all.
    assert_eq!(fixture.audit.taken().len(), 4);

    fixture.store.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 1,
            audit_failures: 0
        })
    );
}

#[tokio::test]
async fn a_release_that_cannot_be_recorded_still_releases_and_says_so() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);

    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            // Two holds and two removals: every record was still attempted.
            audit_failures: 4
        })
    );
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_sink_that_never_answers_costs_each_record_its_own_deadline_and_no_more() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let started = tokio::time::Instant::now();

    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
    // The first record's timeout did not spend the later records' attempts.
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert_eq!(started.elapsed(), Duration::from_secs(20));
    assert!(fixture.store.held().is_empty());
}

#[tokio::test]
async fn a_release_nobody_can_be_named_for_changes_nothing() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    let unused = fixture.ticket(CONVERSATION, b"another file", PDF).await;
    fixture.audit.taken_all();
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);

    for (surface, action) in [(" ", "close-1"), ("phone", ""), ("phone", &"a".repeat(257))] {
        let mut unattributable = release_request(CONVERSATION);
        unattributable.surface_id = surface.into();
        unattributable.correlation_id = action.into();
        assert_eq!(
            fixture.service.release(unattributable).await,
            Err(ReleaseError::Unattributable)
        );
    }
    // Refused before the first effect: the hold, its bytes, and the unused
    // ticket are all as they were, and the sink was never asked.
    assert_eq!(fixture.store.held().len(), 1);
    assert_eq!(fixture.store.blob_count(), 1);
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts);
    fixture
        .service
        .receive(&unused, None, ChannelBody::of(b"another file", 64))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_caller_the_conversation_context_accepts_is_one_this_context_can_record() {
    // The two rules agree, so a close the conversation context admits can
    // never reach a release that has nobody to name.
    for identity in [
        "close-1",
        "close\u{7}1",
        "line\nbreak",
        " padded ",
        "",
        " ",
        "\t\n",
        &"a".repeat(256),
        &"a".repeat(257),
        &"é".repeat(128),
        &"é".repeat(129),
    ] {
        assert_eq!(
            ActionContext::new("closer", identity, identity).is_ok(),
            Caller::new(principal("closer"), identity, identity).is_ok(),
            "{identity:?}"
        );
    }

    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.ticket(CONVERSATION, b"another file", PDF).await;
    fixture.audit.taken_all();
    let mut request = release_request(CONVERSATION);
    request.correlation_id = "close\u{7}1".into();
    fixture.service.release(request).await.unwrap();

    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
    // Everything that was let go is on record, under the identifier as given.
    let records = fixture.audit.taken_all();
    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        let release = match record {
            AttachmentAuditRecord::TicketWithdrawn { release, .. }
            | AttachmentAuditRecord::HoldReleased { release, .. }
            | AttachmentAuditRecord::BlobRemoved { release, .. } => release,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(release.caller.action_id(), "close\u{7}1");
        assert_eq!(release.caller.principal_id(), &principal("closer"));
    }
}

#[tokio::test]
async fn issuing_a_ticket_is_recorded_before_the_ticket_is_handed_out() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(1),
        ..AttachmentLimits::default()
    });
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::Audit)
    );
    // The unrecorded ticket is not left outstanding: the book's one place is free.
    fixture.audit.refusing.store(false, Ordering::SeqCst);
    fixture.clock.set(NOW_MS + 3);
    fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    let records = fixture.audit.taken_all();
    let [AttachmentAuditRecord::TicketIssued { ticket }] = records.as_slice() else {
        panic!("one issuance expected, got {records:?}")
    };
    assert_eq!(ticket.organization_id(), &organization("org"));
    assert_eq!(ticket.conversation_id(), &conversation(OTHER_CONVERSATION));
    assert_eq!(ticket.attachment(), &attachment(BYTES, PDF));
    assert_eq!(ticket.caller().principal_id(), &principal("owner"));
    assert_eq!(ticket.caller().surface_id(), "panel");
    assert_eq!(ticket.caller().action_id(), "begin-1");
    assert_eq!(ticket.lifetime().issued_at_ms(), NOW_MS + 3);
    assert_eq!(
        ticket.lifetime().expires_at_ms(),
        NOW_MS + 3 + TICKET_LIFETIME_MS
    );
}

#[tokio::test]
async fn a_begin_made_again_replaces_its_ticket_and_takes_nobody_elses_place() {
    let fixture = fixture();
    fixture
        .ownership
        .give(OTHER_CONVERSATION, "other-org", "victim");
    let first = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let mut latest = first.clone();
    // Far more repeats than there are tickets in all: each one replaces the last.
    for _ in 0..100 {
        latest = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    }
    assert!(latest != first);
    let replaced = fixture
        .audit
        .taken()
        .iter()
        .filter(|record| matches!(record, AttachmentAuditRecord::TicketReplaced { .. }))
        .count();
    assert_eq!(replaced, 100);
    assert!(matches!(
        fixture
            .service
            .begin(
                caller("other-org", "victim"),
                begin_request(OTHER_CONVERSATION, b"victim file", PDF)
            )
            .await,
        Ok(BeginOutcome::UploadRequired { .. })
    ));
    // Only the latest works.
    assert_eq!(
        fixture
            .service
            .receive(&first, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::TicketInvalid)
    );
    fixture
        .service
        .receive(&latest, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();
}

#[tokio::test]
async fn one_conversation_and_one_organization_are_bounded_below_the_whole_gateway() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: TicketLimits {
            total: 4,
            per_organization: 3,
            per_conversation: 2,
        },
        ..AttachmentLimits::default()
    });
    fixture
        .ownership
        .give("00000000-0000-4000-8000-0000000000a3", "org", "owner");
    fixture.ownership.give(
        "00000000-0000-4000-8000-0000000000b1",
        "other-org",
        "victim",
    );
    // Different action identifiers: different requests, each with a ticket.
    fixture.ticket_as("one", CONVERSATION, BYTES, PDF).await;
    fixture.ticket_as("two", CONVERSATION, BYTES, PDF).await;
    let begin_as = |request_id: &'static str, conversation_id: &'static str| {
        let mut request = begin_request(conversation_id, BYTES, PDF);
        request.request_id = request_id.into();
        fixture.service.begin(caller("org", "owner"), request)
    };
    assert_eq!(
        begin_as("three", CONVERSATION).await,
        Err(BeginError::Capacity)
    );
    begin_as("three", OTHER_CONVERSATION).await.unwrap();
    assert_eq!(
        begin_as("four", "00000000-0000-4000-8000-0000000000a3").await,
        Err(BeginError::Capacity)
    );
    // The organization that filled its share has not filled anyone else's.
    assert!(fixture
        .service
        .begin(
            caller("other-org", "victim"),
            begin_request("00000000-0000-4000-8000-0000000000b1", BYTES, PDF)
        )
        .await
        .is_ok());
}

#[tokio::test]
async fn stale_tickets_are_swept_by_every_begin_even_one_refused_for_its_own_reasons() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(2),
        ..AttachmentLimits::default()
    });
    fixture
        .ticket_as("stale-1", CONVERSATION, b"one", PDF)
        .await;
    fixture
        .ticket_as("stale-2", CONVERSATION, b"two", PDF)
        .await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    // A begin that is refused for its own reasons still clears them out.
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "intruder"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::ConversationNotFound)
    );
    assert_eq!(fixture.audit.taken_all().len(), 2);
}

#[tokio::test]
async fn an_upload_sweeps_the_tickets_nobody_came_back_for() {
    let fixture = fixture();
    fixture
        .ticket_as("stale", CONVERSATION, b"stale", PDF)
        .await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS);
    let live = fixture.ticket_as("live", CONVERSATION, BYTES, PDF).await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    fixture
        .service
        .receive(&live, None, ChannelBody::of(BYTES, 64))
        .await
        .unwrap();
    let records = fixture.audit.taken_all();
    assert!(
        matches!(
            records.as_slice(),
            [
                AttachmentAuditRecord::TicketExpired { ticket },
                AttachmentAuditRecord::HoldCreated { .. }
            ] if ticket.caller().action_id() == "stale"
        ),
        "{records:?}"
    );
}

#[tokio::test]
async fn a_conversation_that_let_go_can_hold_again_until_its_next_close() {
    // Closing is not final: a conversation can be reopened, and nothing in its
    // ownership record says it was closed. So uploads after a close are held
    // like any others, and the next close lets them go.
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    assert!(fixture.store.held().is_empty());
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    assert_eq!(fixture.store.held().len(), 1);
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
}

#[tokio::test]
async fn what_the_normalizer_answers_is_kept_only_if_a_message_could_name_it() {
    let too_large = vec![7_u8; 5 * 1024 * 1024 + 1];
    for answer in [
        StubNormalizer::producing(&too_large, "image/png"),
        StubNormalizer::producing(&too_large, "image/bmp"),
        StubNormalizer::producing(b"small", "image/bmp"),
        StubNormalizer::producing(b"small", "image/heic"),
    ] {
        let fixture = Fixture::with_normalizer(AttachmentLimits::default(), answer);
        let ticket = fixture.ticket(CONVERSATION, BYTES, "image/png").await;
        assert_eq!(
            fixture
                .service
                .receive(&ticket, None, ChannelBody::of(BYTES, 64))
                .await,
            Err(rejected(UploadRejection::NormalizationFailed))
        );
        assert_eq!(fixture.store.blob_count(), 0);
        assert!(fixture.store.held().is_empty());
        assert_eq!(fixture.store.pending(), 0);
        assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
        assert!(matches!(
            fixture.audit.taken().as_slice(),
            [AttachmentAuditRecord::UploadRejected {
                reason: UploadRejection::NormalizationFailed,
                ..
            }]
        ));
    }
    // The largest image a message can name is kept.
    let largest = vec![7_u8; 5 * 1024 * 1024];
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(&largest, "image/webp"),
    );
    let stored = fixture
        .upload(CONVERSATION, BYTES, "image/x-adobe-dng")
        .await;
    assert_eq!(stored, attachment(&largest, "image/webp"));
    assert!(stored.as_image().is_some());
}

#[tokio::test]
async fn a_model_that_is_offered_no_images_is_not_a_bad_image() {
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::failing(NormalizeError::NotOffered),
    );
    let ticket = fixture.ticket(CONVERSATION, BYTES, "image/png").await;
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(rejected(UploadRejection::ImageInputUnsupported))
    );
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
    assert!(matches!(
        fixture.audit.taken().as_slice(),
        [AttachmentAuditRecord::UploadRejected {
            reason: UploadRejection::ImageInputUnsupported,
            ..
        }]
    ));
}

#[tokio::test]
async fn a_hold_that_cannot_be_made_usable_is_taken_back_and_said_to_be() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.store.confirm_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(rejected(UploadRejection::StorageUnavailable))
    );
    // Nothing usable, nothing pending, no bytes; and the trail, which said a
    // hold was created, says next that it did not last.
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(fixture.store.blob_count(), 0);
    let records = fixture.audit.taken();
    assert!(
        matches!(
            records.as_slice(),
            [
                AttachmentAuditRecord::HoldCreated { hold: created },
                AttachmentAuditRecord::HoldReverted {
                    hold: reverted,
                    cause: RevertCause::ConfirmationFailed
                }
            ] if created == reverted
        ),
        "{records:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_creation_the_sink_never_acknowledged_is_followed_by_its_reversal() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    // The sink takes the creation record and never answers: it may commit it
    // yet. Every later record is answered at once.
    let _never_opened = fixture.audit.hold_after(0, false);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::AuditUnavailable { reverted: true })
    );
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(fixture.store.blob_count(), 0);
    // So the trail never ends on "held" for a hold that is gone.
    let records = fixture.audit.taken();
    assert!(
        matches!(
            records.as_slice(),
            [AttachmentAuditRecord::HoldReverted {
                cause: RevertCause::AuditUnconfirmed,
                ..
            }]
        ),
        "{records:?}"
    );
}

#[tokio::test]
async fn an_unrecorded_upload_never_shows_as_stored_and_never_costs_a_later_one_its_hold() {
    let fixture = fixture();
    let first = fixture.ticket_as("begin-1", CONVERSATION, BYTES, PDF).await;
    let second = fixture.ticket_as("begin-2", CONVERSATION, BYTES, PDF).await;
    // The first upload's creation record waits at the sink, and will be refused.
    let open = fixture.audit.hold_after(0, true);
    let service = fixture.service.clone();
    let entered = fixture.audit.entered.notified();
    let one = tokio::spawn(async move {
        service
            .receive(&first, None, ChannelBody::of(BYTES, 7))
            .await
    });
    entered.await;
    // Its hold is written but pending: a begin does not answer "stored" for it.
    assert_eq!(fixture.store.pending(), 1);
    let mut third = begin_request(CONVERSATION, BYTES, PDF);
    third.request_id = "begin-3".into();
    assert!(matches!(
        fixture.service.begin(caller("org", "owner"), third).await,
        Ok(BeginOutcome::UploadRequired { .. })
    ));
    // The second upload of the same file is recorded and acknowledged.
    fixture
        .service
        .receive(&second, None, ChannelBody::of(BYTES, 7))
        .await
        .unwrap();
    open.send(()).unwrap();
    assert_eq!(
        one.await.unwrap(),
        Err(UploadError::AuditUnavailable { reverted: true })
    );
    // Taking back the first undid the first only.
    assert!(fixture
        .service
        .holds(
            &organization("org"),
            &conversation(CONVERSATION),
            &attachment(BYTES, PDF)
        )
        .await
        .unwrap());
    assert_eq!(fixture.store.blob(digest_of(BYTES)).unwrap(), BYTES);
    let reverted = fixture
        .audit
        .taken()
        .iter()
        .filter(|record| matches!(record, AttachmentAuditRecord::HoldReverted { .. }))
        .count();
    assert_eq!(
        reverted, 0,
        "nothing of the first upload was left to take back"
    );
}

#[tokio::test]
async fn a_release_takes_a_pending_hold_with_it_and_the_upload_is_told_it_was_not_kept() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    // The creation record waits at the sink, and will be accepted.
    let open = fixture.audit.hold_after(0, false);
    let service = fixture.service.clone();
    let entered = fixture.audit.entered.notified();
    let upload = tokio::spawn(async move {
        service
            .receive(&ticket, None, ChannelBody::of(BYTES, 7))
            .await
    });
    entered.await;
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    open.send(()).unwrap();
    assert_eq!(upload.await.unwrap(), Err(UploadError::NotKept));
    // Nothing is left in the released conversation, usable or pending.
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(fixture.store.blob_count(), 0);
    let kinds: Vec<_> = fixture
        .audit
        .taken()
        .iter()
        .map(|record| match record {
            AttachmentAuditRecord::HoldReleased { was, .. } => {
                assert_eq!(*was, HoldState::Pending);
                "released"
            }
            AttachmentAuditRecord::BlobRemoved { .. } => "removed",
            AttachmentAuditRecord::HoldCreated { .. } => "created",
            AttachmentAuditRecord::HoldReverted {
                cause: RevertCause::RemovedBeforeUsable,
                ..
            } => "reverted",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(kinds, ["released", "removed", "created", "reverted"]);
}

#[tokio::test]
async fn a_file_the_conversation_already_keeps_is_said_to_be_already_kept() {
    // Two different photographs that normalize to the same image.
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(b"same result", "image/png"),
    );
    let kept = fixture
        .upload(CONVERSATION, b"first photograph", "image/heic")
        .await;
    let before = fixture.store.held();
    fixture.audit.taken_all();
    fixture.clock.set(NOW_MS + 50);
    let ticket = fixture
        .ticket_as("begin-2", CONVERSATION, b"second photograph", "image/heic")
        .await;
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(b"second photograph", 64))
            .await
            .unwrap(),
        kept
    );
    // The hold is exactly the one that was there, first upload's facts and all,
    // and the record says nothing was created.
    assert_eq!(fixture.store.held(), before);
    assert_eq!(fixture.store.pending(), 0);
    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::AlreadyHeld { ticket, hold }] = records.as_slice() else {
        panic!("one record expected, got {records:?}")
    };
    assert_eq!(
        ticket.attachment(),
        &attachment(b"second photograph", "image/heic")
    );
    assert_eq!(ticket.caller().action_id(), "begin-2");
    assert_eq!(hold, &before[0]);
    assert_eq!(
        hold.uploaded(),
        &attachment(b"first photograph", "image/heic")
    );
}

/// Counts how many images are being normalized at once, and holds each until
/// the test lets it through.
struct GatedNormalizer {
    active: AtomicUsize,
    peak: AtomicUsize,
    entered: Notify,
    gate: Semaphore,
}
impl ImageNormalizer for GatedNormalizer {
    fn normalize<'a>(&'a self, original: Vec<u8>, _: &'a str) -> NormalizeFuture<'a> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.entered.notify_one();
            let _ = self
                .gate
                .acquire()
                .await
                .map_err(|_| NormalizeError::Failed)?;
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(NormalizedImage {
                bytes: original,
                media_type: "image/png".into(),
            })
        })
    }
}

#[tokio::test]
async fn images_are_normalized_fewer_at_a_time_than_they_are_transferred() {
    let store = Arc::new(MemoryStore::default());
    let normalizer = Arc::new(GatedNormalizer {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        entered: Notify::new(),
        gate: Semaphore::new(0),
    });
    let ownership = Arc::new(FixedOwnership::default());
    ownership.give(CONVERSATION, "org", "owner");
    let service = AttachmentService::new(
        AttachmentDependencies {
            store: store.clone(),
            audit: Arc::new(RecordingAudit::default()),
            ownership,
            secrets: Arc::new(CountingSecrets::default()),
            normalizer: normalizer.clone(),
            clock: Arc::new(ManualClock::at(NOW_MS)),
        },
        AttachmentLimits {
            max_uploads: 3,
            max_normalizations: 1,
            ..AttachmentLimits::default()
        },
    );
    let mut uploads = Vec::new();
    for (request_id, bytes) in [("one", b"first image"), ("two", b"other image")] {
        let mut request = begin_request(CONVERSATION, bytes, "image/x-canon-cr3");
        request.request_id = request_id.into();
        let BeginOutcome::UploadRequired { ticket, .. } = service
            .begin(caller("org", "owner"), request)
            .await
            .unwrap()
        else {
            panic!("a ticket was expected")
        };
        let service = service.clone();
        uploads.push(tokio::spawn(async move {
            service
                .receive(&ticket.expose(), None, ChannelBody::of(bytes, 64))
                .await
        }));
    }
    normalizer.entered.notified().await;
    // Both transfers are complete and safe on disk; only one image is in memory.
    while store.written.load(Ordering::SeqCst) != 22 {
        tokio::task::yield_now().await;
    }
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert_eq!(normalizer.active.load(Ordering::SeqCst), 1);
    normalizer.gate.add_permits(2);
    for upload in uploads {
        upload.await.unwrap().unwrap();
    }
    assert_eq!(normalizer.peak.load(Ordering::SeqCst), 1);
    assert_eq!(store.held().len(), 2);
}
