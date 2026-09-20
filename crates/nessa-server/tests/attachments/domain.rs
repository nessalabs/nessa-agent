//! Attachment rules with no store, clock, socket, or runtime anywhere near them.
use super::*;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::domain::{
    agent_execution::prompts::ImageReference,
    common::value_objects::{ImageMediaType, Sha256Digest},
};

const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000a1";
const OTHER_CONVERSATION: &str = "00000000-0000-4000-8000-0000000000a2";

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}
fn file(byte: u8, media_type: &str, size: u64) -> Attachment {
    Attachment::new(digest(byte), MediaType::parse(media_type).unwrap(), size).unwrap()
}
fn caller() -> Caller {
    Caller::new(PrincipalId::new("owner").unwrap(), "panel", "begin-1").unwrap()
}
fn ticket(conversation: &str, attachment: Attachment, issued_at_ms: u64) -> UploadTicket {
    UploadTicket::new(
        OrganizationId::new("org").unwrap(),
        ConversationId::new(conversation).unwrap(),
        attachment,
        caller(),
        TicketLifetime::starting(issued_at_ms).unwrap(),
    )
}
fn fingerprint(byte: u8) -> TicketFingerprint {
    TicketFingerprint::from_bytes([byte; 32])
}

#[test]
fn a_media_type_is_exactly_what_the_wire_pattern_accepts() {
    for accepted in [
        "image/png",
        "application/vnd.api+json",
        "text/x-c++",
        "a/b",
        "1a/2b",
    ] {
        assert_eq!(MediaType::parse(accepted).unwrap().as_str(), accepted);
    }
    let too_long = format!("image/{}", "a".repeat(122));
    assert_eq!(too_long.len(), 128);
    for refused in [
        "",
        "image",
        "image/",
        "/png",
        "Image/png",
        "image/PNG",
        "image/png; charset=utf-8",
        " image/png",
        "image/png ",
        "image/p ng",
        "image//png",
        "image/png/extra",
        ".image/png",
        "image/+png",
        "imagé/png",
        too_long.as_str(),
    ] {
        assert_eq!(
            MediaType::parse(refused),
            Err(AttachmentError::MediaType),
            "accepted {refused:?}"
        );
    }
    assert!(MediaType::parse(&too_long[..127]).is_ok());
    assert!(MediaType::parse("image/svg+xml").unwrap().is_image());
    assert!(!MediaType::parse("application/image").unwrap().is_image());
}

#[test]
fn an_attachment_is_one_byte_to_twenty_mebibytes() {
    let png = || MediaType::parse("image/png").unwrap();
    for refused in [0, Attachment::MAX_BYTES + 1, u64::MAX] {
        assert_eq!(
            Attachment::new(digest(1), png(), refused),
            Err(AttachmentError::Size)
        );
    }
    for accepted in [1, Attachment::MAX_BYTES] {
        assert_eq!(
            Attachment::new(digest(1), png(), accepted).unwrap().size(),
            accepted
        );
    }
    assert_eq!(Attachment::MAX_BYTES, 20 * 1024 * 1024);
}

#[test]
fn an_image_reference_names_the_same_file_the_store_describes() {
    let image = ImageReference::new(digest(7), ImageMediaType::Webp, 9).unwrap();
    assert_eq!(Attachment::of_image(&image), file(7, "image/webp", 9));
}

#[test]
fn a_caller_is_bounded_plain_text_and_nothing_else() {
    let owner = || PrincipalId::new("owner").unwrap();
    for (surface, action) in [
        ("", "a"),
        (" ", "a"),
        ("panel", ""),
        ("panel", "line\nbreak"),
        ("panel\u{0}", "a"),
    ] {
        assert_eq!(
            Caller::new(owner(), surface, action),
            Err(AttachmentError::Caller)
        );
    }
    let longest = "a".repeat(256);
    assert!(Caller::new(owner(), &longest, &longest).is_ok());
    assert!(Caller::new(owner(), "panel", &"a".repeat(257)).is_err());
    // 256 bytes, not 256 characters.
    assert!(Caller::new(owner(), "panel", &"é".repeat(129)).is_err());
}

#[test]
fn a_ticket_lives_five_minutes_and_is_refused_only_after_its_last_millisecond() {
    let lifetime = TicketLifetime::starting(1_000).unwrap();
    assert_eq!(lifetime.issued_at_ms(), 1_000);
    assert_eq!(lifetime.expires_at_ms(), 1_000 + TICKET_LIFETIME_MS);
    assert_eq!(TICKET_LIFETIME_MS, 300_000);
    assert!(lifetime.is_usable_at(1_000));
    assert!(lifetime.is_usable_at(1_000 + TICKET_LIFETIME_MS - 1));
    assert!(lifetime.is_usable_at(1_000 + TICKET_LIFETIME_MS));
    assert!(!lifetime.is_usable_at(1_000 + TICKET_LIFETIME_MS + 1));
    assert!(!lifetime.is_usable_at(u64::MAX));
    // A clock that went backwards proves nothing about expiry.
    assert!(lifetime.is_usable_at(0));
    assert_eq!(
        TicketLifetime::starting(u64::MAX - TICKET_LIFETIME_MS + 1),
        Err(AttachmentError::Lifetime)
    );
    assert!(TicketLifetime::starting(u64::MAX - TICKET_LIFETIME_MS).is_ok());
}

#[test]
fn received_bytes_must_be_exactly_the_described_file() {
    let ticket = ticket(CONVERSATION, file(1, "image/png", 10), 0);
    assert_eq!(ticket.check_received(10, digest(1)), Ok(()));
    assert_eq!(
        ticket.check_received(9, digest(1)),
        Err(UploadMismatch::Size)
    );
    assert_eq!(
        ticket.check_received(11, digest(1)),
        Err(UploadMismatch::Size)
    );
    assert_eq!(
        ticket.check_received(10, digest(2)),
        Err(UploadMismatch::Digest)
    );
    // Both wrong is a size mismatch: a digest of the wrong bytes says nothing.
    assert_eq!(
        ticket.check_received(11, digest(2)),
        Err(UploadMismatch::Size)
    );
    assert!(!ticket.is_exceeded_by(10));
    assert!(ticket.is_exceeded_by(11));
}

#[test]
fn a_hold_answers_only_for_the_owner_conversation_and_file_it_names() {
    let ticket = ticket(CONVERSATION, file(1, "image/heic", 10), 50);
    let hold = Hold::from_upload(&ticket, file(2, "image/jpeg", 8), 60).unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let conversation = ConversationId::new(CONVERSATION).unwrap();
    assert!(hold.keeps(&organization, &conversation, &file(2, "image/jpeg", 8)));
    assert!(hold.came_from(&organization, &conversation, &file(1, "image/heic", 10)));
    assert_eq!(hold.uploaded_by(), &caller());
    assert_eq!(hold.ticket_issued_at_ms(), 50);
    assert_eq!(hold.uploaded_at_ms(), 60);

    // Each of these is valid on its own and wrong in one fact only.
    for stored in [
        file(3, "image/jpeg", 8),
        file(2, "image/png", 8),
        file(2, "image/jpeg", 9),
        // The uploaded file is not what is kept, and must not pass for it.
        file(1, "image/heic", 10),
    ] {
        assert!(
            !hold.keeps(&organization, &conversation, &stored),
            "{stored:?}"
        );
    }
    for uploaded in [
        file(9, "image/heic", 10),
        file(1, "image/png", 10),
        file(1, "image/heic", 11),
        file(2, "image/jpeg", 8),
    ] {
        assert!(
            !hold.came_from(&organization, &conversation, &uploaded),
            "{uploaded:?}"
        );
    }
    let other_organization = OrganizationId::new("Org").unwrap();
    let other_conversation = ConversationId::new(OTHER_CONVERSATION).unwrap();
    let stored = file(2, "image/jpeg", 8);
    assert!(!hold.keeps(&other_organization, &conversation, &stored));
    assert!(!hold.keeps(&organization, &other_conversation, &stored));
    assert!(!hold.came_from(&other_organization, &conversation, hold.uploaded()));
    assert!(!hold.came_from(&organization, &other_conversation, hold.uploaded()));
}

#[test]
fn only_an_image_may_be_stored_as_something_else_and_it_stays_an_image() {
    let text = ticket(CONVERSATION, file(1, "text/plain", 10), 0);
    assert!(Hold::from_upload(&text, file(1, "text/plain", 10), 1).is_some());
    for changed in [
        file(2, "text/plain", 10),
        file(1, "text/markdown", 10),
        file(1, "text/plain", 9),
        file(2, "image/png", 5),
    ] {
        assert!(Hold::from_upload(&text, changed, 1).is_none());
    }
    let image = ticket(CONVERSATION, file(1, "image/png", 10), 0);
    assert!(Hold::from_upload(&image, file(1, "image/png", 10), 1).is_some());
    assert!(Hold::from_upload(&image, file(2, "image/jpeg", 4), 1).is_some());
    assert!(Hold::from_upload(&image, file(2, "application/pdf", 4), 1).is_none());
}

#[test]
fn a_ticket_leaves_the_book_once_and_only_by_its_own_fingerprint() {
    let mut book = TicketBook::new(4);
    let first = ticket(CONVERSATION, file(1, "image/png", 10), 0);
    let second = ticket(OTHER_CONVERSATION, file(2, "image/png", 10), 0);
    book.issue(fingerprint(1), first.clone()).unwrap();
    book.issue(fingerprint(2), second.clone()).unwrap();

    assert_eq!(book.redeem(&fingerprint(3), 1), Redemption::Unknown);
    assert_eq!(book.outstanding(), 2);
    assert_eq!(book.redeem(&fingerprint(2), 1), Redemption::Usable(second));
    assert_eq!(book.redeem(&fingerprint(2), 1), Redemption::Unknown);
    assert_eq!(book.outstanding(), 1);
    assert_eq!(book.redeem(&fingerprint(1), 1), Redemption::Usable(first));
    assert_eq!(book.outstanding(), 0);
}

#[test]
fn an_expired_ticket_is_returned_as_expired_whether_presented_or_swept() {
    let mut book = TicketBook::new(4);
    let early = ticket(CONVERSATION, file(1, "image/png", 10), 0);
    let late = ticket(CONVERSATION, file(2, "image/png", 10), 1_000);
    book.issue(fingerprint(1), early.clone()).unwrap();
    book.issue(fingerprint(2), late.clone()).unwrap();

    assert!(book.expire(TICKET_LIFETIME_MS).is_empty());
    // Presented one millisecond late: it leaves the book, as expired, once.
    assert_eq!(
        book.redeem(&fingerprint(1), TICKET_LIFETIME_MS + 1),
        Redemption::Expired(early)
    );
    assert_eq!(
        book.redeem(&fingerprint(1), TICKET_LIFETIME_MS + 1),
        Redemption::Unknown
    );
    assert!(book.expire(TICKET_LIFETIME_MS + 1_000).is_empty());
    assert_eq!(book.expire(TICKET_LIFETIME_MS + 1_001), vec![late]);
    assert!(book.expire(u64::MAX).is_empty());
    assert_eq!(book.outstanding(), 0);
}

#[test]
fn a_full_book_refuses_new_tickets_until_old_ones_are_accounted_for() {
    let mut book = TicketBook::new(2);
    for byte in [1, 2] {
        book.issue(
            fingerprint(byte),
            ticket(CONVERSATION, file(byte, "image/png", 10), 0),
        )
        .unwrap();
    }
    let third = || ticket(CONVERSATION, file(3, "image/png", 10), u64::MAX / 2);
    assert_eq!(book.issue(fingerprint(3), third()), Err(BookFull));
    // Expired tickets still count until their expiry has been handed over.
    assert_eq!(book.issue(fingerprint(3), third()), Err(BookFull));
    assert_eq!(book.expire(u64::MAX / 2).len(), 2);
    book.issue(fingerprint(3), third()).unwrap();
    assert_eq!(book.outstanding(), 1);
    assert_eq!(
        TicketBook::new(0).issue(fingerprint(1), third()),
        Err(BookFull)
    );
}

#[test]
fn a_conversation_that_lets_go_takes_its_own_tickets_with_it_and_nobody_elses() {
    let mut book = TicketBook::new(8);
    let mine = ticket(CONVERSATION, file(1, "image/png", 10), 0);
    let mine_too = ticket(CONVERSATION, file(2, "image/png", 10), 5);
    let theirs = ticket(OTHER_CONVERSATION, file(1, "image/png", 10), 0);
    let foreign = UploadTicket::new(
        OrganizationId::new("other-org").unwrap(),
        ConversationId::new(CONVERSATION).unwrap(),
        file(1, "image/png", 10),
        caller(),
        TicketLifetime::starting(0).unwrap(),
    );
    for (byte, issued) in [(1, &mine), (2, &theirs), (3, &mine_too), (4, &foreign)] {
        book.issue(fingerprint(byte), issued.clone()).unwrap();
    }
    let organization = OrganizationId::new("org").unwrap();
    let conversation = ConversationId::new(CONVERSATION).unwrap();
    assert_eq!(book.void(&organization, &conversation), [mine, mine_too]);
    assert!(book.void(&organization, &conversation).is_empty());
    // Gone for good, and the others untouched.
    assert_eq!(book.redeem(&fingerprint(1), 1), Redemption::Unknown);
    assert_eq!(book.redeem(&fingerprint(2), 1), Redemption::Usable(theirs));
    assert_eq!(book.redeem(&fingerprint(4), 1), Redemption::Usable(foreign));
}

#[test]
fn a_fingerprint_matches_only_itself_and_never_prints_itself() {
    assert!(fingerprint(1).matches(&fingerprint(1)));
    assert!(!fingerprint(1).matches(&fingerprint(2)));
    let mut nearly = [1_u8; 32];
    nearly[31] = 0;
    assert!(!fingerprint(1).matches(&TicketFingerprint::from_bytes(nearly)));
    assert_eq!(format!("{:?}", fingerprint(0xab)), "TicketFingerprint(..)");
}
