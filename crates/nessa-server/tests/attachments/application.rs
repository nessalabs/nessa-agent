//! The attachment service over doubles: tickets, uploads, normalization,
//! release, and what reaches the audit port when each of them fails.
use super::*;
use crate::attachments::application::{
    ReleaseEvidence, ReleaseReport, RemovedBlob, RetiredHold, RetirementEvidence,
};
use crate::attachments::domain::{
    Attachment, Caller, Hold, HoldState, MediaType, RetiredFrom, TicketLifetime, TicketLimits,
    UploadTicket, TICKET_LIFETIME_MS,
};
use crate::attachments::infrastructure::DurableAttachmentAudit;
use crate::attachments_test_support::{
    attachment, begin_request, caller, conversation, digest_of, organization, principal,
    ChannelBody, CountingSecrets, FixedOwnership, Fixture, ManualClock, MemoryStore,
    RecordingAudit, StubNormalizer, CONVERSATION, NOW_MS, OTHER_CONVERSATION,
};
use futures_util::FutureExt;
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use serde_json::Value;
use std::{
    io::Write,
    panic::AssertUnwindSafe,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, PoisonError,
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
/// Nothing was kept, nothing is pending, nothing is still staged, and the one
/// record says why, of the file and caller the ticket named.
fn assert_refused(fixture: &Fixture, media_type: &str, reason: UploadRejection) {
    assert_eq!(fixture.store.blob_count(), 0);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
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
    assert_eq!(ticket.attachment(), &attachment(BYTES, media_type));
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
    // already disagrees before a byte is read. The last is declared an image,
    // which the normalizer must never be shown: nothing is prepared from bytes
    // that have not proved to be the file the ticket described.
    for (media_type, sent, declared, reason) in [
        (PDF, &BYTES[..19], None, UploadRejection::SizeMismatch),
        (PDF, BYTES, Some(21), UploadRejection::SizeMismatch),
        (PDF, BYTES, Some(0), UploadRejection::SizeMismatch),
        (
            "image/png",
            b"twenty bytes of FILE".as_slice(),
            None,
            UploadRejection::DigestMismatch,
        ),
    ] {
        let fixture = Fixture::with_normalizer(
            AttachmentLimits::default(),
            StubNormalizer::producing(b"small", "image/png"),
        );
        let ticket = fixture.ticket(CONVERSATION, BYTES, media_type).await;
        assert_eq!(
            fixture
                .service
                .receive(&ticket, declared, ChannelBody::of(sent, 4))
                .await,
            Err(rejected(reason)),
            "{declared:?}"
        );
        assert_refused(&fixture, media_type, reason);
        assert!(fixture.normalizer.seen.lock().unwrap().is_empty());
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
    assert_refused(&fixture, PDF, UploadRejection::SizeMismatch);
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
    assert_refused(&fixture, PDF, UploadRejection::UploadInterrupted);
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
    fixture.store.wrote(10).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    // ...and then the connection it was reading from closes.
    sender.send(Err(UploadInterrupted)).unwrap();
    fixture.audit.recorded.notified().await;
    assert_refused(&fixture, PDF, UploadRejection::UploadInterrupted);
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
    fixture.store.wrote(10).await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!upload.is_finished());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        upload.await.unwrap(),
        Err(rejected(UploadRejection::UploadTimeout))
    );
    assert_refused(&fixture, PDF, UploadRejection::UploadTimeout);
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
    fixture.store.wrote(10).await;
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
    assert_refused(&fixture, PDF, UploadRejection::StorageUnavailable);

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

#[tokio::test(start_paused = true)]
async fn a_hold_that_cannot_be_recorded_is_taken_back() {
    // The sink refuses the creation record, and the sink takes it and never
    // answers, which may yet commit it: the same failure either way, and the
    // trail never ends on "held" for a hold that is gone.
    for never_answers in [false, true] {
        let fixture = fixture();
        let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
        fixture.audit.taken_all();
        let door = fixture.audit.hold_after(0, true);
        let _held_open = never_answers.then_some(door);
        assert_eq!(
            fixture
                .service
                .receive(&ticket, None, ChannelBody::of(BYTES, 64))
                .await,
            Err(UploadError::AuditUnavailable)
        );
        assert!(fixture.store.held().is_empty());
        assert_eq!(fixture.store.pending(), 0);
        assert_eq!(fixture.store.blob_count(), 0);
        assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
        let records = fixture.audit.taken();
        assert!(
            matches!(
                records.as_slice(),
                [AttachmentAuditRecord::HoldReverted {
                    cause: RevertCause::AuditUnconfirmed,
                    ..
                }]
            ),
            "{never_answers}: {records:?}"
        );
    }

    // Bytes another conversation already holds survive the taking back, and a
    // hold that could not be taken back is left pending, which nothing sees.
    let fixture = fixture();
    fixture.upload(OTHER_CONVERSATION, BYTES, PDF).await;
    let again = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    fixture.store.discard_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&again, None, ChannelBody::of(BYTES, 64))
            .await,
        Err(UploadError::AuditUnavailable)
    );
    fixture.audit.refusing.store(false, Ordering::SeqCst);
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
    // The normalizer was shown the bytes that arrived, once.
    assert_eq!(
        *fixture.normalizer.seen.lock().unwrap(),
        [original.to_vec()]
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
    // Every way preparing an image can fail, in one table: what the normalizer
    // refuses, and what it answers that no message could ever name. It is
    // outside code, so what it hands back is checked as hard as what it reads.
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
        // A ticket for an image only exists where images are offered, so a
        // normalizer that then says it prepares none is a broken normalizer.
        (
            StubNormalizer::failing(NormalizeError::NotOffered),
            UploadRejection::ImageInputUnsupported,
        ),
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
        // Not an encoding a message may name, and over a message's per-image
        // size: the hold's own rule refuses both.
        (
            StubNormalizer::producing(b"small", "image/bmp"),
            UploadRejection::NormalizationFailed,
        ),
        (
            StubNormalizer::producing(&vec![7_u8; 5 * 1024 * 1024 + 1], "image/png"),
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
        assert_refused(&fixture, "image/png", reason);
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
async fn a_gateway_offered_no_images_issues_no_ticket_for_one() {
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::offering_nothing(),
    );
    for media_type in ["image/png", "image/heic", "image/x-canon-cr3"] {
        assert_eq!(
            fixture
                .service
                .begin(
                    caller("org", "owner"),
                    begin_request(CONVERSATION, BYTES, media_type)
                )
                .await,
            // Not a bad file: a perfectly good one, for a model offered none.
            Err(BeginError::ImagesUnsupported),
            "{media_type}"
        );
    }
    // Nothing was permitted, so nothing was recorded and no secret was spent.
    assert!(fixture.audit.taken_all().is_empty());
    // Anything else is stored as ever: the gateway takes files, the model
    // takes images, and only the second of those is missing here.
    let stored = fixture.upload(CONVERSATION, BYTES, PDF).await;
    assert_eq!(stored, attachment(BYTES, PDF));
    assert!(fixture.normalizer.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_release_withdraws_the_conversations_unused_tickets_in_the_closers_name() {
    let fixture = fixture();
    let withdrawn = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let untouched = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();

    // Only this conversation's ticket, and the caller it was issued to is
    // part of what was withdrawn rather than who withdrew it.
    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::TicketWithdrawn { ticket, release }] = records.as_slice() else {
        panic!("one withdrawal expected, got {records:?}")
    };
    assert_eq!(ticket.conversation_id(), &conversation(CONVERSATION));
    assert_eq!(ticket.caller().principal_id(), &principal("owner"));
    assert_eq!(release.caller.principal_id(), &principal("closer"));
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
async fn an_acknowledged_release_returns_when_the_records_are_written() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let started = tokio::time::Instant::now();

    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();

    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_release_records(&fixture.audit.taken());
    assert!(fixture.store.held().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_refusing_sink_is_attempted_for_every_record_without_spending_the_budget() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let started = tokio::time::Instant::now();

    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_release_still_attempts_every_record_for_its_own_deadline() {
    // Two holds and their two removals. The budget pays for two deadlines and
    // a bit of a third; it must not shorten that third, and it must not skip
    // the fourth. The caller is what the budget bounds.
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(12),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    fixture.audit.clear_durations();
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
    assert_eq!(started.elapsed(), Duration::from_secs(12));
    // Two attempts have used a full deadline. The third is in flight and was
    // not cut off when the caller returned.
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 3);
    assert_eq!(
        fixture.audit.durations(),
        vec![Duration::from_secs(5), Duration::from_secs(5)]
    );
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 3);
    assert_eq!(fixture.audit.durations().len(), 2);

    // Past the fourth deadline. A sleep that lands on it can be polled before
    // the attempt's own timer, and then the drop has not been timed yet.
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert_eq!(fixture.audit.durations(), vec![Duration::from_secs(5); 4]);
}

#[tokio::test(start_paused = true)]
async fn a_stalled_expiry_sweep_still_attempts_every_ticket() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(4),
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(7),
        ..AttachmentLimits::default()
    });
    for request_id in ["one", "two", "three"] {
        fixture
            .ticket_as(request_id, CONVERSATION, request_id.as_bytes(), PDF)
            .await;
    }
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    fixture.audit.clear_durations();
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let started = tokio::time::Instant::now();

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
    assert_eq!(started.elapsed(), Duration::from_secs(7));
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);
    assert_eq!(fixture.audit.durations(), vec![Duration::from_secs(5)]);

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);
    assert_eq!(fixture.audit.durations().len(), 1);

    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 3);
    assert_eq!(fixture.audit.durations(), vec![Duration::from_secs(5); 3]);
}

#[tokio::test(start_paused = true)]
async fn expired_tickets_free_their_places_before_the_sweep_is_acknowledged() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(4),
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(7),
        ..AttachmentLimits::default()
    });
    for request_id in ["one", "two", "three"] {
        fixture
            .ticket_as(request_id, CONVERSATION, request_id.as_bytes(), PDF)
            .await;
    }
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    fixture.audit.stalled.store(true, Ordering::SeqCst);

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
    // One expiry attempt is still inside its own deadline. The tickets are
    // already gone, so the book can issue again.
    fixture.audit.stalled.store(false, Ordering::SeqCst);
    for request_id in ["four", "five", "six", "seven"] {
        fixture
            .ticket_as(request_id, CONVERSATION, request_id.as_bytes(), PDF)
            .await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_lost_release_caller_does_not_cancel_the_remaining_attempts() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();

    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) == attempts + 4 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert_release_records(&fixture.audit.taken());
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_record_acknowledged_after_the_caller_gave_up_stays_a_failure() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(10),
        audit_budget: Duration::from_secs(1),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let started = tokio::time::Instant::now();

    let result = fixture.service.release(release_request(CONVERSATION)).await;

    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(
        result,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
    assert!(fixture.audit.taken_all().is_empty());
    gate.send(()).unwrap();
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) == attempts + 4 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    assert_release_records(&fixture.audit.taken());
    assert_eq!(
        result,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
}

#[tokio::test(start_paused = true)]
async fn bulk_delivery_does_not_admit_more_than_its_limit() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        audit_admission: 2,
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    fixture.audit.clear_durations();
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let release = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });

    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= attempts + 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    let result = release.await.unwrap();
    assert_eq!(
        result,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
    assert_eq!(fixture.audit.durations(), vec![Duration::from_secs(5); 4]);
}

#[tokio::test(start_paused = true)]
async fn a_second_bulk_phase_waits_for_the_admission_permit() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        audit_admission: 1,
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(OTHER_CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let first = fixture.service.clone();
    let second = fixture.service.clone();
    let first = tokio::spawn(async move { first.release(release_request(CONVERSATION)).await });
    let second =
        tokio::spawn(async move { second.release(release_request(OTHER_CONVERSATION)).await });

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 1);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);

    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
}

#[tokio::test(start_paused = true)]
async fn a_second_phase_enters_when_the_permit_is_released() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        audit_admission: 1,
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(OTHER_CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let before = fixture.audit.conversations().len();
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let first = fixture.service.clone();
    let second = fixture.service.clone();
    let first = tokio::spawn(async move { first.release(release_request(CONVERSATION)).await });
    let second =
        tokio::spawn(async move { second.release(release_request(OTHER_CONVERSATION)).await });

    tokio::time::sleep(Duration::from_secs(1)).await;
    let seen = fixture.audit.conversations();
    assert_eq!(&seen[before..], &[conversation(CONVERSATION)]);
    // One second past the deadline. The permit has been released, and the
    // other phase was already waiting on it.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let seen = fixture.audit.conversations();
    let bulk = &seen[before..];
    assert!(
        bulk.len() >= 2,
        "the second record was not handed over: {bulk:?}"
    );
    assert_eq!(
        bulk[1],
        conversation(OTHER_CONVERSATION),
        "the first phase's remaining record was queued ahead of the waiting phase: {bulk:?}"
    );

    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
}

#[tokio::test(start_paused = true)]
async fn a_zero_admission_still_attempts_every_record() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(1),
        audit_admission: 0,
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let started = tokio::time::Instant::now();

    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();

    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(fixture.audit.taken().len(), 2);
}

#[tokio::test]
async fn a_closed_admission_does_not_hand_the_record_to_the_sink() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    fixture.service.close_bulk_admission_for_test();

    assert_eq!(
        fixture.service.release(release_request(CONVERSATION)).await,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 2
        })
    );
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts);
    assert!(fixture.store.held().is_empty());
}

#[tokio::test]
async fn dropping_the_service_stops_bulk_attempts_that_have_not_started() {
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let audit = Arc::clone(&fixture.audit);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    drop(fixture);
    let _ = gate.send(());
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert_eq!(audit.attempts.load(Ordering::SeqCst), attempts + 1);
}

#[tokio::test]
async fn a_panicked_delivery_does_not_drop_the_next_phase_s_records() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(OTHER_CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);

    // The other conversation still has its hold. Its release must own those
    // records before the parked panic is resumed. Beginning again would sweep,
    // and an empty sweep would resume that panic before any new records existed.
    let before = fixture.audit.attempts.load(Ordering::SeqCst);
    let caught = AssertUnwindSafe(fixture.service.release(release_request(OTHER_CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(caught.is_err());
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= before + 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), before + 2);
}

#[tokio::test]
async fn an_empty_phase_surfaces_a_panicked_delivery() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);

    let caught = AssertUnwindSafe(fixture.service.release(release_request(OTHER_CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(caught.is_err());
}

#[tokio::test]
async fn a_parked_bulk_panic_does_not_reject_a_later_upload() {
    let fixture = fixture();
    // Issued first. begin is a phase caller, so issuing after the panic is
    // parked would resume it here instead of inside the upload.
    let ticket = fixture.ticket(OTHER_CONVERSATION, BYTES, PDF).await;
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= attempts + 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);

    let stored = fixture
        .service
        .receive(&ticket, Some(BYTES.len() as u64), ChannelBody::of(BYTES, 7))
        .await
        .unwrap();
    assert_eq!(stored, attachment(BYTES, PDF));
    let records = fixture.audit.taken();
    let [AttachmentAuditRecord::HoldCreated { hold }] = records.as_slice() else {
        panic!("the upload was kept, got {records:?}");
    };
    assert_eq!(hold.stored(), &attachment(BYTES, PDF));
    assert_eq!(hold.conversation_id(), &conversation(OTHER_CONVERSATION));

    // The upload did not await the parked task, so a later release still
    // surfaces that panic.
    let caught = AssertUnwindSafe(fixture.service.release(release_request(CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(caught.is_err());
}

#[tokio::test]
async fn a_second_parked_panic_is_resumed_by_a_later_release() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, b"first", PDF).await;
    fixture.upload(OTHER_CONVERSATION, b"second", PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let first_service = fixture.service.clone();
    let second_service = fixture.service.clone();
    let first =
        tokio::spawn(async move { first_service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    let second = tokio::spawn(async move {
        second_service
            .release(release_request(OTHER_CONVERSATION))
            .await
    });
    // Let the second phase reach the semaphore before its caller is dropped.
    // Aborting sooner cancels release before it parks a delivery task.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
    gate.send(()).unwrap();
    for _ in 0..64 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= attempts + 4 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 4);
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);

    let first_surface = AssertUnwindSafe(fixture.service.release(release_request(CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(first_surface.is_err());
    let second_surface =
        AssertUnwindSafe(fixture.service.release(release_request(OTHER_CONVERSATION)))
            .catch_unwind()
            .await;
    assert!(
        second_surface.is_err(),
        "reap dropped the second parked panic while resuming the first"
    );
}

#[tokio::test]
async fn a_sweep_panic_parked_beside_an_older_one_is_still_surfaced() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(4),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    let _stale = fixture.ticket(OTHER_CONVERSATION, b"stale", PDF).await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS - 1);
    let fresh = fixture
        .ticket_as("begin-2", OTHER_CONVERSATION, b"fresh", PDF)
        .await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= attempts + 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);
    // The sweep's expiry record panics. The upload's own record does not.
    fixture.audit.panic_next.store(1, Ordering::SeqCst);

    let stored = fixture
        .service
        .receive(&fresh, Some(5), ChannelBody::of(b"fresh", 7))
        .await
        .unwrap();
    assert_eq!(stored, attachment(b"fresh", PDF));

    let first_surface = AssertUnwindSafe(fixture.service.release(release_request(CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(first_surface.is_err());
    let second_surface =
        AssertUnwindSafe(fixture.service.release(release_request(OTHER_CONVERSATION)))
            .catch_unwind()
            .await;
    assert!(
        second_surface.is_err(),
        "the sweep panic was dropped while the older panic was resumed"
    );
}

#[tokio::test]
async fn a_panicked_record_does_not_skip_the_rest_of_its_phase() {
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    fixture.audit.panic_in_record.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) >= attempts + 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), attempts + 2);
    fixture.audit.panic_in_record.store(false, Ordering::SeqCst);

    let caught = AssertUnwindSafe(fixture.service.release(release_request(OTHER_CONVERSATION)))
        .catch_unwind()
        .await;
    assert!(caught.is_err());
}

#[derive(Clone)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
impl LogCapture {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    fn text(&self) -> String {
        String::from_utf8(
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .unwrap()
    }
}
impl Write for LogCapture {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

fn error_log() -> (LogCapture, tracing::subscriber::DefaultGuard) {
    let captured = LogCapture::new();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::ERROR)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, guard)
}

#[tokio::test]
async fn a_lost_caller_still_logs_a_refusal() {
    let (captured, _guard) = error_log();
    let fixture = fixture();
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, true);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    fixture.audit.entered.notified().await;
    caller.abort();
    let _ = caller.await;
    gate.send(()).unwrap();
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        captured
            .text()
            .contains("attachment audit record was refused"),
        "{}",
        captured.text()
    );
}

#[tokio::test(start_paused = true)]
async fn a_lost_caller_still_logs_a_deadline() {
    let (captured, _guard) = error_log();
    let fixture = Fixture::new(AttachmentLimits {
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(30),
        ..AttachmentLimits::default()
    });
    fixture.upload(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    fixture.audit.stalled.store(true, Ordering::SeqCst);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);
    let service = fixture.service.clone();
    let caller = tokio::spawn(async move { service.release(release_request(CONVERSATION)).await });
    for _ in 0..32 {
        if fixture.audit.attempts.load(Ordering::SeqCst) > attempts {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(fixture.audit.attempts.load(Ordering::SeqCst) > attempts);
    caller.abort();
    let _ = caller.await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(
        captured
            .text()
            .contains("attachment audit record was not acknowledged before its deadline"),
        "{}",
        captured.text()
    );
}

#[tokio::test(start_paused = true)]
async fn an_upload_proceeds_while_an_expiry_sweep_is_still_unacknowledged() {
    let fixture = Fixture::new(AttachmentLimits {
        tickets: tickets(4),
        audit_deadline: Duration::from_secs(5),
        audit_budget: Duration::from_secs(1),
        ..AttachmentLimits::default()
    });
    let _stale = fixture.ticket(CONVERSATION, b"stale", PDF).await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS - 1);
    let fresh = fixture
        .ticket_as("begin-2", OTHER_CONVERSATION, b"fresh", PDF)
        .await;
    fixture.clock.set(NOW_MS + TICKET_LIFETIME_MS + 1);
    fixture.audit.taken_all();
    let gate = fixture.audit.hold_after(0, false);
    let attempts = fixture.audit.attempts.load(Ordering::SeqCst);

    let stored = fixture
        .service
        .receive(&fresh, Some(5), ChannelBody::of(b"fresh", 7))
        .await
        .unwrap();

    assert_eq!(stored, attachment(b"fresh", PDF));
    assert!(fixture.audit.attempts.load(Ordering::SeqCst) > attempts);
    assert!(fixture
        .audit
        .taken()
        .iter()
        .any(|record| matches!(record, AttachmentAuditRecord::HoldCreated { .. })));
    gate.send(()).unwrap();
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(fixture
        .audit
        .taken_all()
        .iter()
        .any(|record| matches!(record, AttachmentAuditRecord::TicketExpired { .. })));
}

/// A bulk sink whose write keeps running after the service's deadline. The
/// service holds admission; this sink never sees that permit. Single-record
/// calls return at once until measuring is turned on.
struct DetachedAudit {
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    measured_attempts: Arc<AtomicUsize>,
    /// Set once the uploads are done, so only the bulk release is measured.
    measure: Arc<AtomicBool>,
}
impl AttachmentAudit for DetachedAudit {
    fn record(&self, _record: AttachmentAuditRecord) -> PortFuture<'_, (), AuditUnavailable> {
        let measure = self.measure.load(Ordering::SeqCst);
        let in_flight = Arc::clone(&self.in_flight);
        let max_in_flight = Arc::clone(&self.max_in_flight);
        let measured_attempts = Arc::clone(&self.measured_attempts);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                if !measure {
                    return Ok(());
                }
                measured_attempts.fetch_add(1, Ordering::SeqCst);
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_in_flight.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(400));
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .map_err(|_| AuditUnavailable)?
        })
    }
}

#[tokio::test]
async fn a_bulk_write_that_outlives_its_deadline_releases_the_admission_slot() {
    let store = Arc::new(MemoryStore::default());
    let audit = Arc::new(DetachedAudit {
        in_flight: Arc::new(AtomicUsize::new(0)),
        max_in_flight: Arc::new(AtomicUsize::new(0)),
        measured_attempts: Arc::new(AtomicUsize::new(0)),
        measure: Arc::new(AtomicBool::new(false)),
    });
    let ownership = Arc::new(FixedOwnership::default());
    ownership.give(CONVERSATION, "org", "owner");
    let service = AttachmentService::new(
        AttachmentDependencies {
            store: store.clone(),
            audit: audit.clone(),
            ownership,
            secrets: Arc::new(CountingSecrets::default()),
            normalizer: Arc::new(StubNormalizer::failing(NormalizeError::Failed)),
            clock: Arc::new(ManualClock::at(NOW_MS)),
        },
        AttachmentLimits {
            audit_deadline: Duration::from_millis(80),
            audit_budget: Duration::from_millis(30),
            audit_admission: 1,
            ..AttachmentLimits::default()
        },
    );
    let fixture_service = service.clone();
    // Two held files, so release hands over four records.
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let ticket = match fixture_service
            .begin(caller("org", "owner"), {
                let mut request = begin_request(CONVERSATION, bytes, PDF);
                request.request_id = format!("begin-{}", bytes[0]);
                request
            })
            .await
            .unwrap()
        {
            BeginOutcome::UploadRequired { ticket, .. } => ticket.expose(),
            BeginOutcome::Stored(_) => panic!("a ticket was expected"),
        };
        fixture_service
            .receive(&ticket, Some(bytes.len() as u64), ChannelBody::of(bytes, 7))
            .await
            .unwrap();
    }
    audit.measure.store(true, Ordering::SeqCst);
    let result = service.release(release_request(CONVERSATION)).await;
    assert_eq!(
        result,
        Err(ReleaseError::Incomplete {
            storage_failures: 0,
            audit_failures: 4
        })
    );
    for _ in 0..80 {
        if audit.measured_attempts.load(Ordering::SeqCst) == 4
            && audit.in_flight.load(Ordering::SeqCst) == 0
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(audit.measured_attempts.load(Ordering::SeqCst), 4);
    assert_eq!(audit.in_flight.load(Ordering::SeqCst), 0);
    assert!(
        audit.max_in_flight.load(Ordering::SeqCst) >= 2,
        "a write that outlived its deadline still held the only slot"
    );
}

/// Both uploads were released in the closer's name, for the close that asked.
fn assert_release_records(records: &[AttachmentAuditRecord]) {
    assert_eq!(records.len(), 4, "{records:?}");
    for record in records {
        let release = match record {
            AttachmentAuditRecord::HoldReleased { hold, was, release } => {
                assert_eq!(*was, HoldState::Held);
                assert_eq!(hold.uploaded_by().principal_id(), &principal("owner"));
                release
            }
            AttachmentAuditRecord::BlobRemoved { removed } => {
                assert_eq!(removed.retirements().len(), 1);
                let RetirementEvidence::Release(release) = &removed.retirements()[0].evidence()
                else {
                    panic!("expected explicit release")
                };
                release
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(release.caller.action_id(), "close-1");
        assert_eq!(release.caller.principal_id(), &principal("closer"));
        assert_eq!(release.caller.surface_id(), "phone");
        assert_eq!(release.cause, ReleaseCause::ConversationClosed);
    }
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
    fixture.clock.set(NOW_MS + 9);
    let mut request = release_request(CONVERSATION);
    request.correlation_id = "close\u{7}1".into();
    fixture.service.release(request).await.unwrap();

    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.blob_count(), 0);
    // Everything that was let go is on record, under the identifier as given,
    // and in the closer's name rather than the uploader's.
    let records = fixture.audit.taken_all();
    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        let release = match record {
            AttachmentAuditRecord::TicketWithdrawn { release, .. } => release,
            AttachmentAuditRecord::HoldReleased { hold, was, release } => {
                assert_eq!(*was, HoldState::Held);
                assert_eq!(hold.uploaded_by().principal_id(), &principal("owner"));
                release
            }
            AttachmentAuditRecord::BlobRemoved { removed } => {
                assert_eq!(removed.retirements().len(), 1);
                let retired = &removed.retirements()[0];
                assert_eq!(retired.hold().stored().digest(), digest_of(BYTES));
                let RetirementEvidence::Release(release) = &retired.evidence() else {
                    panic!("expected explicit release")
                };
                release
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(release.caller.action_id(), "close\u{7}1");
        assert_eq!(release.caller.principal_id(), &principal("closer"));
        assert_eq!(release.caller.surface_id(), "phone");
        assert_eq!(release.cause, ReleaseCause::ConversationClosed);
        assert_eq!(release.requested_at_ms, NOW_MS + 9);
    }

    // This MemoryStore removes completed records and returns an empty report.
    // The real store retains retirement metadata; its retry evidence is covered
    // by the actual durable-audit fixtures in artifacts.rs.
    fixture
        .service
        .release(release_request(CONVERSATION))
        .await
        .unwrap();
    assert!(fixture.audit.taken_all().is_empty());
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
                    cause: RevertCause::ConfirmationFailed, was: RetiredFrom::Pending, }
            ] if created == reverted
        ),
        "{records:?}"
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
async fn an_upload_whose_own_work_stops_still_accounts_for_the_ticket_and_the_hold() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    fixture.audit.taken_all();
    // The ticket is spent, the bytes are published, the hold is written and
    // recorded, and then the work stops with nothing to answer with.
    fixture.store.confirm_panics.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 7))
            .await,
        Err(UploadError::Rejected {
            reason: UploadRejection::Unresolved,
            evidence: AuditDelivery::Recorded,
        })
    );
    // Nothing is left of it: not a usable hold, not a pending one, not bytes.
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(fixture.store.blob_count(), 0);
    assert_eq!(fixture.store.staged.load(Ordering::SeqCst), 0);
    // And the trail says what happened rather than naming some other failure:
    // the hold was taken back because its upload never came back, and the
    // ticket was used without producing one.
    let records = fixture.audit.taken_all();
    assert!(
        matches!(
            records.as_slice(),
            [
                AttachmentAuditRecord::HoldCreated { .. },
                AttachmentAuditRecord::HoldReverted {
                    cause: RevertCause::UploadUnresolved,
                    ..
                },
                AttachmentAuditRecord::UploadRejected {
                    reason: UploadRejection::Unresolved,
                    ticket,
                },
            ] if ticket.caller().action_id() == "begin-1"
        ),
        "{records:?}"
    );
    // The ticket was spent by that attempt, whatever it did or did not do.
    fixture.store.confirm_panics.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&ticket, None, ChannelBody::of(BYTES, 7))
            .await,
        Err(UploadError::TicketInvalid)
    );
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

    // Unrecorded, arriving at a file the conversation already keeps is still a
    // refusal, and nothing was created to take back: the hold that was there
    // stays exactly as it was, and the upload is not told anything was undone.
    let again = fixture
        .ticket_as("begin-3", CONVERSATION, b"third photograph", "image/heic")
        .await;
    fixture.audit.taken_all();
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture
            .service
            .receive(&again, None, ChannelBody::of(b"third photograph", 64))
            .await,
        Err(UploadError::AuditUnavailable)
    );
    fixture.audit.refusing.store(false, Ordering::SeqCst);
    assert_eq!(fixture.store.held(), before);
    assert_eq!(fixture.store.pending(), 0);
    assert!(fixture
        .service
        .holds(&organization("org"), &conversation(CONVERSATION), &kept)
        .await
        .unwrap());
    // Nothing claims a hold was reverted, because none was made.
    assert!(fixture.audit.taken_all().is_empty());
}

#[tokio::test]
async fn a_hold_another_upload_took_over_is_not_this_uploads_to_keep_or_to_undo() {
    // The hold this upload wrote is confirmed by the upload of the same file
    // that overtook it, and then this one's own confirmation fails. It has
    // nothing left to take back and nothing of its own missing from the trail,
    // so it is told the file was not kept rather than that evidence was lost.
    let fixture = fixture();
    let first = fixture.ticket_as("begin-1", CONVERSATION, BYTES, PDF).await;
    let second = fixture.ticket_as("begin-2", CONVERSATION, BYTES, PDF).await;
    let open = fixture.audit.hold_after(0, false);
    let entered = fixture.audit.entered.notified();
    let service = fixture.service.clone();
    let one = tokio::spawn(async move {
        service
            .receive(&first, None, ChannelBody::of(BYTES, 7))
            .await
    });
    entered.await;
    // The second upload takes the pending hold over and makes it usable.
    fixture
        .service
        .receive(&second, None, ChannelBody::of(BYTES, 7))
        .await
        .unwrap();
    fixture.store.confirm_fails.store(true, Ordering::SeqCst);
    open.send(()).unwrap();
    assert_eq!(one.await.unwrap(), Err(UploadError::NotKept));

    fixture.store.confirm_fails.store(false, Ordering::SeqCst);
    assert!(fixture
        .service
        .holds(
            &organization("org"),
            &conversation(CONVERSATION),
            &attachment(BYTES, PDF)
        )
        .await
        .unwrap());
    let reverted = fixture
        .audit
        .taken()
        .iter()
        .filter(|record| matches!(record, AttachmentAuditRecord::HoldReverted { .. }))
        .count();
    assert_eq!(reverted, 0, "nothing of the first upload was left to undo");
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
    fn offers_images(&self) -> bool {
        true
    }
    fn normalize(&self, original: Vec<u8>) -> NormalizeFuture<'_> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            self.entered.notify_one();
            // Each permit lets exactly one image through, so the test decides
            // how many are let through and when.
            self.gate
                .acquire()
                .await
                .map_err(|_| NormalizeError::Failed)?
                .forget();
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
    // One image is in memory. The other cannot reach the normalizer at all
    // while it is, so its turn comes only when the first one's is over.
    normalizer.entered.notified().await;
    assert_eq!(normalizer.active.load(Ordering::SeqCst), 1);
    let entered = normalizer.entered.notified();
    normalizer.gate.add_permits(1);
    entered.await;
    assert_eq!(normalizer.active.load(Ordering::SeqCst), 1);
    normalizer.gate.add_permits(1);
    for upload in uploads {
        upload.await.unwrap().unwrap();
    }
    assert_eq!(normalizer.peak.load(Ordering::SeqCst), 1);
    assert_eq!(store.held().len(), 2);
}

#[tokio::test]
async fn a_deleted_conversation_begins_no_upload() {
    let fixture = fixture();
    fixture
        .ownership
        .deleted
        .lock()
        .unwrap()
        .push(conversation(CONVERSATION));
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "owner"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::ConversationDeleted)
    );
    // Somebody else is told only that there is no such conversation.
    assert_eq!(
        fixture
            .service
            .begin(
                caller("org", "stranger"),
                begin_request(CONVERSATION, BYTES, PDF)
            )
            .await,
        Err(BeginError::ConversationNotFound)
    );
    // No ticket was issued, so none was recorded.
    assert!(fixture.audit.taken_all().is_empty());
}

#[tokio::test]
async fn an_upload_whose_conversation_is_no_longer_found_is_reverted_as_not_found() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    let service = fixture.service.clone();
    let receiving = tokio::spawn(async move { service.receive(&ticket, None, body).await });
    fixture.store.wrote(10).await;

    // While it transfers, ownership stops saying the conversation is the
    // uploader's — and nothing says it was deleted.
    fixture.audit.taken_all();
    fixture
        .ownership
        .owners
        .lock()
        .unwrap()
        .remove(&conversation(CONVERSATION));
    sender.send(Ok(BYTES[10..].to_vec())).unwrap();
    drop(sender);
    assert_eq!(receiving.await.unwrap(), Err(UploadError::NotKept));
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    // The trail says what was found, not a deletion nobody made.
    let records = fixture.audit.taken_all();
    assert!(
        matches!(
            records.as_slice(),
            [
                AttachmentAuditRecord::HoldCreated { .. },
                AttachmentAuditRecord::HoldReverted {
                    cause: RevertCause::ConversationNotFound,
                    ..
                }
            ]
        ),
        "{records:?}"
    );
}

#[tokio::test]
async fn an_upload_finishing_after_its_conversation_was_deleted_keeps_nothing() {
    let fixture = fixture();
    let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
    let (sender, body) = ChannelBody::open();
    sender.send(Ok(BYTES[..10].to_vec())).unwrap();
    let service = fixture.service.clone();
    let receiving = tokio::spawn(async move { service.receive(&ticket, None, body).await });
    fixture.store.wrote(10).await;

    // The conversation is deleted while its upload is still transferring: the
    // tombstone first, then the release, which finds no hold yet to let go of.
    fixture
        .ownership
        .deleted
        .lock()
        .unwrap()
        .push(conversation(CONVERSATION));
    fixture
        .service
        .release(ReleaseRequest {
            cause: ReleaseCause::ConversationDeleted,
            ..release_request(CONVERSATION)
        })
        .await
        .unwrap();
    fixture.audit.taken_all();

    // The rest arrives, and the hold it would have written is taken back.
    sender.send(Ok(BYTES[10..].to_vec())).unwrap();
    drop(sender);
    assert_eq!(receiving.await.unwrap(), Err(UploadError::NotKept));
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(fixture.store.blob_count(), 0);
    // The trail says the hold was created and then that it did not last, and why.
    let records = fixture.audit.taken_all();
    assert!(
        matches!(
            records.as_slice(),
            [
                AttachmentAuditRecord::HoldCreated { .. },
                AttachmentAuditRecord::HoldReverted {
                    cause: RevertCause::ConversationDeleted,
                    ..
                }
            ]
        ),
        "{records:?}"
    );
}

#[tokio::test]
async fn a_retired_hold_with_incomplete_blob_cleanup_keeps_the_independent_audit_result() {
    for refuse_reversal_audit in [false, true] {
        let fixture = fixture();
        let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
        fixture.store.confirm_fails.store(true, Ordering::SeqCst);
        fixture
            .store
            .discard_cleanup_incomplete
            .store(true, Ordering::SeqCst);
        let door = fixture.audit.hold_after(1, refuse_reversal_audit);
        let service = fixture.service.clone();
        let entered = fixture.audit.entered.notified();
        let upload = tokio::spawn(async move {
            service
                .receive(&ticket, None, ChannelBody::of(BYTES, 64))
                .await
        });
        entered.await;
        door.send(()).unwrap();
        let evidence = if refuse_reversal_audit {
            AuditDelivery::Unavailable
        } else {
            AuditDelivery::Recorded
        };
        assert_eq!(
            upload.await.unwrap(),
            Err(UploadError::Rejected {
                reason: UploadRejection::StorageUnavailable,
                evidence,
            })
        );
        assert!(fixture.store.held().is_empty());
        assert_eq!(fixture.store.pending(), 0);
        assert_eq!(fixture.store.blob_count(), 1);
        let records = fixture.audit.taken();
        if refuse_reversal_audit {
            assert!(matches!(
                records.as_slice(),
                [AttachmentAuditRecord::HoldCreated { .. }]
            ));
        } else {
            assert!(
                matches!(records.as_slice(), [AttachmentAuditRecord::HoldCreated { hold: created }, AttachmentAuditRecord::HoldReverted { hold: reverted, cause: RevertCause::ConfirmationFailed, was: RetiredFrom::Pending, }] if created == reverted)
            );
        }
    }
}

#[tokio::test]
async fn pending_and_held_retirements_preserve_independent_cleanup_and_audit_results() {
    for retired_from in [RetiredFrom::Pending, RetiredFrom::Held] {
        for cleanup_incomplete in [false, true] {
            for refuse_reversal_audit in [false, true] {
                let fixture = fixture();
                let ticket = fixture.ticket(CONVERSATION, BYTES, PDF).await;
                fixture
                    .store
                    .confirm_fails
                    .store(retired_from == RetiredFrom::Pending, Ordering::SeqCst);
                fixture
                    .store
                    .confirm_reply_fails
                    .store(retired_from == RetiredFrom::Held, Ordering::SeqCst);
                fixture
                    .store
                    .discard_cleanup_incomplete
                    .store(cleanup_incomplete, Ordering::SeqCst);
                let door = fixture.audit.hold_after(1, refuse_reversal_audit);
                door.send(()).unwrap();
                let evidence = if refuse_reversal_audit {
                    AuditDelivery::Unavailable
                } else {
                    AuditDelivery::Recorded
                };
                assert_eq!(
                    fixture
                        .service
                        .receive(&ticket, None, ChannelBody::of(BYTES, 64))
                        .await,
                    Err(UploadError::Rejected {
                        reason: UploadRejection::StorageUnavailable,
                        evidence
                    })
                );
                assert!(fixture.store.held().is_empty());
                assert_eq!(fixture.store.pending(), 0);
                assert_eq!(fixture.store.blob_count(), usize::from(cleanup_incomplete));
                assert_eq!(fixture.audit.attempts.load(Ordering::SeqCst), 3);
                let records = fixture.audit.taken();
                if refuse_reversal_audit {
                    assert!(matches!(
                        records.as_slice(),
                        [AttachmentAuditRecord::HoldCreated { .. }]
                    ));
                } else {
                    assert!(
                        matches!(records.as_slice(), [AttachmentAuditRecord::HoldCreated { hold: created }, AttachmentAuditRecord::HoldReverted { hold: reverted, cause: RevertCause::ConfirmationFailed, was }] if created == reverted && *was == retired_from),
                        "{records:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn release_validates_admitted_target_and_cross_entry_agreement_before_hold_audit() {
    let hold_for = |org: &str, conversation_id: &str, media: &str| {
        let stored = attachment(BYTES, media);
        let ticket = UploadTicket::new(
            organization(org),
            conversation(conversation_id),
            stored.clone(),
            Caller::new(principal("owner"), "panel", "original-upload").unwrap(),
            TicketLifetime::starting(1_000).unwrap(),
        );
        Hold::from_upload(&ticket, stored, 2_000).unwrap()
    };
    let original = ReleaseEvidence {
        cause: ReleaseCause::ConversationDeleted,
        caller: Caller::new(
            principal("original-closer"),
            "original-panel",
            "original-release",
        )
        .unwrap(),
        requested_at_ms: 3_000,
    };
    let retirement = |hold| {
        RetiredHold::new(
            hold,
            RetiredFrom::Held,
            RetirementEvidence::Release(original.clone()),
        )
        .unwrap()
    };
    let one = retirement(hold_for("org", CONVERSATION, PDF));
    let two = retirement(hold_for("org", CONVERSATION, "image/png"));
    let foreign_org = retirement(hold_for("foreign", CONVERSATION, PDF));
    let foreign_conversation = retirement(hold_for("org", OTHER_CONVERSATION, PDF));
    let changed = RetiredHold::new(
        one.hold().clone(),
        RetiredFrom::Pending,
        RetirementEvidence::Release(original.clone()),
    )
    .unwrap();
    let removal = |entries| RemovedBlob::new(entries).unwrap();
    let invalid = [
        ReleaseReport {
            retired: vec![foreign_org.clone()],
            removed: vec![],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![foreign_conversation.clone()],
            removed: vec![],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![],
            removed: vec![removal(vec![foreign_org])],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone()],
            removed: vec![removal(vec![foreign_conversation])],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone(), one.clone()],
            removed: vec![],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone()],
            removed: vec![removal(vec![one.clone()]), removal(vec![one.clone()])],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone()],
            removed: vec![removal(vec![changed])],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone(), two.clone()],
            removed: vec![removal(vec![one.clone()])],
            failures: 0,
        },
        ReleaseReport {
            retired: vec![one.clone(), two.clone()],
            removed: vec![removal(vec![one.clone(), one.clone()])],
            failures: 0,
        },
    ];
    for (case, report) in invalid.into_iter().enumerate() {
        let fixture = fixture();
        fixture.ticket(CONVERSATION, BYTES, PDF).await;
        *fixture.store.release_report.lock().unwrap() = Some(report);
        assert_eq!(
            fixture.service.release(release_request(CONVERSATION)).await,
            Err(ReleaseError::Incomplete {
                storage_failures: 1,
                audit_failures: 0
            }),
            "case {case}"
        );
        let records = fixture.audit.taken();
        assert!(
            matches!(
                records.as_slice(),
                [AttachmentAuditRecord::TicketWithdrawn { .. }]
            ),
            "case {case}: {records:?}"
        );
    }
    for reverse in [false, true] {
        let fixture = fixture();
        let entries = if reverse {
            vec![two.clone(), one.clone()]
        } else {
            vec![one.clone(), two.clone()]
        };
        *fixture.store.release_report.lock().unwrap() = Some(ReleaseReport {
            retired: entries.clone(),
            removed: vec![removal(entries)],
            failures: 0,
        });
        fixture
            .service
            .release(release_request(CONVERSATION))
            .await
            .unwrap();
        let records = fixture.audit.taken();
        assert_eq!(records.len(), 3);
        for record in &records {
            match record {
                AttachmentAuditRecord::HoldReleased { release, .. } => {
                    assert_eq!(release, &original)
                }
                AttachmentAuditRecord::BlobRemoved { removed } => {
                    assert_eq!(removed.retirements().len(), 2)
                }
                _ => panic!("unexpected audit {record:?}"),
            }
        }
    }
}

#[tokio::test]
async fn release_report_stored_lengths_agree_before_actual_durable_audit() {
    let retired = |media: &str, size, normalized| {
        let stored =
            Attachment::new(digest_of(b"bytes"), MediaType::parse(media).unwrap(), size).unwrap();
        let uploaded = if normalized {
            attachment(
                if media == "image/png" {
                    b"a different original image length"
                } else {
                    b"another image"
                },
                "image/png",
            )
        } else {
            stored.clone()
        };
        let ticket = UploadTicket::new(
            organization("org"),
            conversation(CONVERSATION),
            uploaded,
            Caller::new(principal("uploader"), "panel", "upload-1").unwrap(),
            TicketLifetime::starting(1_000).unwrap(),
        );
        RetiredHold::new(
            Hold::from_upload(&ticket, stored, 2_000).unwrap(),
            RetiredFrom::Held,
            RetirementEvidence::Release(ReleaseEvidence {
                cause: ReleaseCause::ConversationDeleted,
                caller: Caller::new(principal("original-closer"), "panel", "original-release")
                    .unwrap(),
                requested_at_ms: 3_000,
            }),
        )
        .unwrap()
    };
    for reverse in [false, true] {
        let first = retired(PDF, 5, false);
        let second = retired("text/plain", 6, false);
        let entries = if reverse {
            vec![second, first]
        } else {
            vec![first, second]
        };
        assert!(
            RemovedBlob::new(entries.clone()).is_none(),
            "contradictory content length cannot construct a removal group"
        );
        for partial_removal in [false, true] {
            let fixture = fixture();
            let audit = tempfile::tempdir().unwrap();
            let audit_path = audit.path().join("records");
            let service = AttachmentService::new(
                AttachmentDependencies {
                    store: fixture.store.clone(),
                    audit: Arc::new(
                        DurableAttachmentAudit::new(audit_path.clone(), fixture.clock.clone())
                            .unwrap(),
                    ),
                    ownership: fixture.ownership.clone(),
                    secrets: fixture.secrets.clone(),
                    normalizer: fixture.normalizer.clone(),
                    clock: fixture.clock.clone(),
                },
                AttachmentLimits::default(),
            );
            *fixture.store.release_report.lock().unwrap() = Some(ReleaseReport {
                retired: entries.clone(),
                removed: if partial_removal {
                    vec![RemovedBlob::new(vec![entries[0].clone()]).unwrap()]
                } else {
                    vec![]
                },
                failures: 0,
            });
            assert_eq!(
                service.release(release_request(CONVERSATION)).await,
                Err(ReleaseError::Incomplete {
                    storage_failures: 1,
                    audit_failures: 0
                })
            );
            assert_eq!(
                std::fs::read_dir(&audit_path).unwrap().count(),
                0,
                "false content must not become durable audit"
            );
        }
        // Same stored content with different image declarations and uploaded
        // lengths is legitimate normalization, not a content contradiction.
        let first = retired("image/png", 5, true);
        let second = retired("image/jpeg", 5, true);
        assert_ne!(first.hold().uploaded().size(), first.hold().stored().size());
        let entries = if reverse {
            vec![second, first]
        } else {
            vec![first, second]
        };
        for removed in [false, true] {
            let fixture = fixture();
            let audit = tempfile::tempdir().unwrap();
            let audit_path = audit.path().join("records");
            let service = AttachmentService::new(
                AttachmentDependencies {
                    store: fixture.store.clone(),
                    audit: Arc::new(
                        DurableAttachmentAudit::new(audit_path.clone(), fixture.clock.clone())
                            .unwrap(),
                    ),
                    ownership: fixture.ownership.clone(),
                    secrets: fixture.secrets.clone(),
                    normalizer: fixture.normalizer.clone(),
                    clock: fixture.clock.clone(),
                },
                AttachmentLimits::default(),
            );
            *fixture.store.release_report.lock().unwrap() = Some(ReleaseReport {
                retired: entries.clone(),
                removed: if removed {
                    vec![RemovedBlob::new(entries.clone()).unwrap()]
                } else {
                    vec![]
                },
                failures: 0,
            });
            service
                .release(release_request(CONVERSATION))
                .await
                .unwrap();
            let records: Vec<Value> = std::fs::read_dir(&audit_path)
                .unwrap()
                .map(|entry| {
                    serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
                })
                .collect();
            assert_eq!(records.len(), if removed { 3 } else { 2 });
            for record in &records {
                if record["kind"] == "attachment_hold_released" {
                    assert_eq!(record["correlationId"], "original-release");
                }
            }
        }
    }
}
