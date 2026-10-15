//! Canonical bytes and the outcome a gateway answer becomes.
use super::{
    after_lookup, after_send, history_reaches, outranks, Cause, Delivery, Initiator, IntentError,
    IntentEvent, LookupAnswer, PhoneCommandId, PhoneFailure, PhoneImage, PhoneIntent, PhoneOutcome,
    PhoneRecord, ReceiptEffect, ReceiptStage, SendAnswer, UnknownCause, FOREGROUND_SENDS,
};
use nessa_protocol::product_contract::generated::{ConversationErrorCode, MAX_MCP_MESSAGE_BYTES};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

const CONVERSATION: &str = "01234567-89ab-4cde-8f01-23456789abcd";

fn command(value: &str) -> PhoneCommandId {
    PhoneCommandId::new(value).unwrap()
}

fn turn(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}

fn intent(text: &str) -> PhoneIntent {
    PhoneIntent::new(
        CONVERSATION,
        command("command-1"),
        turn("turn-1"),
        text,
        Vec::new(),
    )
    .unwrap()
}

fn image() -> PhoneImage {
    PhoneImage::new(
        Sha256Digest::parse(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        ImageMediaType::parse("image/png").unwrap(),
        4,
    )
    .unwrap()
}

fn departed(text: &str) -> PhoneRecord {
    PhoneRecord::held(intent(text)).depart().unwrap()
}

#[test]
fn canonical_bytes_round_trip_and_keep_surrounding_whitespace() {
    let saved = intent("  hello  ");
    let restored = PhoneIntent::from_canonical(saved.canonical()).unwrap();
    assert_eq!(restored.text(), "  hello  ");
    assert_eq!(restored.canonical(), saved.canonical());
    assert!(saved.same_bytes(&restored));
}

#[test]
fn an_image_without_text_is_one_send() {
    let saved = PhoneIntent::new(
        CONVERSATION,
        command("command-1"),
        turn("turn-1"),
        "   ",
        vec![image()],
    )
    .unwrap();
    assert_eq!(saved.text(), "   ");
    assert_eq!(saved.images().len(), 1);
    let restored = PhoneIntent::from_canonical(saved.canonical()).unwrap();
    assert_eq!(restored.images()[0].media_type(), "image/png");
    assert_eq!(restored.images()[0].size(), 4);
}

#[test]
fn a_blank_send_and_an_overlong_text_are_refused() {
    let blank = PhoneIntent::new(
        CONVERSATION,
        command("command-1"),
        turn("turn-1"),
        " \n",
        Vec::new(),
    );
    assert_eq!(blank.unwrap_err(), IntentError::Empty);
    let overlong = "x".repeat(MAX_MCP_MESSAGE_BYTES + 1);
    let too_long = PhoneIntent::new(
        CONVERSATION,
        command("command-1"),
        turn("turn-1"),
        overlong,
        Vec::new(),
    );
    assert_eq!(too_long.unwrap_err(), IntentError::TooLong);
}

#[test]
fn trailing_canonical_bytes_are_refused() {
    let saved = intent("hello");
    let mut tampered = saved.canonical().to_vec();
    tampered.push(1);
    assert_eq!(
        PhoneIntent::from_canonical(&tampered).unwrap_err(),
        IntentError::Bytes
    );
}

#[test]
fn a_draft_does_not_claim_submission() {
    let view = PhoneRecord::held(intent("hello")).view();
    assert_eq!(view.outcome(), PhoneOutcome::Draft);
    assert!(!view.claims_submission());
    assert!(PhoneOutcome::Accepted.claims_submission());
    assert!(!PhoneOutcome::Unknown(UnknownCause::NotAdmitted).claims_submission());
    assert!(!PhoneOutcome::Failed(PhoneFailure::Forbidden).claims_submission());
}

#[test]
fn a_lost_answer_may_resend_and_an_unreadable_one_may_not() {
    let lost = after_send(departed("hello"), SendAnswer::Lost);
    assert!(matches!(lost, super::Step::Lookup { resend: true, .. }));
    let broken = after_send(departed("hello"), SendAnswer::Broken);
    assert!(matches!(broken, super::Step::Lookup { resend: false, .. }));
    let unresolved = after_send(
        departed("hello"),
        SendAnswer::Refused(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionUnresolved,
        )),
    );
    match unresolved {
        super::Step::Done(record) => {
            assert_eq!(
                record.outcome(),
                PhoneOutcome::Unknown(UnknownCause::Unresolved)
            );
            assert_eq!(record.sends(), 1);
        }
        other => panic!("unresolved must stop, got {other:?}"),
    }
}

#[test]
fn an_absent_receipt_is_not_sent_again_outside_the_foreground_call() {
    let record = departed("hello").with_sends(1);
    let stopped = after_lookup(record.clone(), LookupAnswer::NotFound, false);
    assert!(matches!(stopped, super::Step::Done(_)));
    let continued = after_lookup(record, LookupAnswer::NotFound, true);
    assert!(matches!(continued, super::Step::Send(_)));
    let spent = after_lookup(
        departed("hello").with_sends(FOREGROUND_SENDS),
        LookupAnswer::NotFound,
        true,
    );
    assert!(matches!(spent, super::Step::Done(_)));
    let attempted = departed("hello").with_outcome(PhoneOutcome::Unknown(UnknownCause::Attempted));
    let kept = after_lookup(attempted.clone(), LookupAnswer::NotFound, false);
    assert!(matches!(
        kept,
        super::Step::Done(ref saved)
            if saved.outcome() == PhoneOutcome::Unknown(UnknownCause::Attempted)
    ));
    let settled = after_lookup(
        attempted,
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Dispatched),
        },
        false,
    );
    assert!(matches!(
        settled,
        super::Step::Done(ref saved) if saved.outcome() == PhoneOutcome::Done
    ));
}

#[test]
fn a_settled_receipt_becomes_done_or_the_matching_refusal() {
    let record = departed("hello");
    let done = after_lookup(
        record.clone(),
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Dispatched),
        },
        false,
    );
    assert!(matches!(
        done,
        super::Step::Done(ref saved) if saved.outcome() == PhoneOutcome::Done
    ));
    let revoked = after_lookup(
        record,
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::NoEffect),
        },
        true,
    );
    assert!(matches!(
        revoked,
        super::Step::Done(ref saved)
            if saved.outcome()
                == PhoneOutcome::Failed(PhoneFailure::Conversation(ConversationErrorCode::Revoked))
    ));
}

#[test]
fn a_history_that_stops_before_the_row_does_not_reach_it() {
    let done = departed("hello").with_outcome(PhoneOutcome::Done);
    let stopped = [
        audit(Cause::Held, PhoneOutcome::Draft),
        audit(
            Cause::Departed,
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted),
        ),
    ];
    assert!(!history_reaches(&done, &stopped));
    let accepted = departed("hello")
        .with_sends(1)
        .with_outcome(PhoneOutcome::Accepted);
    let reached = [
        audit(Cause::Held, PhoneOutcome::Draft),
        audit(
            Cause::Departed,
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted),
        ),
        audit(Cause::SendAnswered, PhoneOutcome::Accepted),
    ];
    assert!(history_reaches(&accepted, &reached));
    assert!(outranks(
        PhoneOutcome::Accepted,
        PhoneOutcome::Unknown(UnknownCause::LookupFailed)
    ));
    assert!(!outranks(PhoneOutcome::Accepted, PhoneOutcome::Done));
}

fn audit(cause: Cause, outcome: PhoneOutcome) -> IntentEvent {
    IntentEvent {
        cause,
        outcome,
        initiator: Initiator::Person,
        observed_at_ms: 1,
    }
}

#[test]
fn a_queued_receipt_is_accepted() {
    let step = after_send(departed("hello"), SendAnswer::Receipt(Delivery::Queued));
    assert!(matches!(
        step,
        super::Step::Done(ref saved) if saved.outcome() == PhoneOutcome::Accepted
    ));
}
