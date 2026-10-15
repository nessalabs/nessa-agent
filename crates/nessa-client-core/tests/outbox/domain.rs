//! Canonical bytes and the outcome a gateway answer becomes.
use super::{
    after_lookup, after_send, history_reaches, replaces, stored_evidence, submit_outcome, Cause,
    Delivery, Evidence, Initiator, IntentError, IntentEvent, LookupAnswer, PhoneCommandId,
    PhoneFailure, PhoneImage, PhoneIntent, PhoneOutcome, PhoneRecord, ReceiptEffect, ReceiptStage,
    SendAnswer, UnknownCause, FOREGROUND_SENDS,
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
    assert!(!history_reaches(&done, &stopped, false));
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
    assert!(history_reaches(&accepted, &reached, false));
    assert!(replaces(
        PhoneOutcome::Accepted,
        Evidence::Direct,
        PhoneOutcome::Unknown(UnknownCause::LookupFailed),
        Evidence::Lookup,
    ));
    assert!(!replaces(
        PhoneOutcome::Accepted,
        Evidence::Direct,
        PhoneOutcome::Done,
        Evidence::Direct,
    ));
}

#[test]
fn every_transition_and_precedence_pair() {
    let done = PhoneOutcome::Done;
    let running = PhoneOutcome::Running;
    let accepted = PhoneOutcome::Accepted;
    let failed = PhoneOutcome::Failed(PhoneFailure::Forbidden);
    let unresolved = PhoneOutcome::Unknown(UnknownCause::Unresolved);
    let attempted = PhoneOutcome::Unknown(UnknownCause::Attempted);
    let lookup_failed = PhoneOutcome::Unknown(UnknownCause::LookupFailed);
    let not_admitted = PhoneOutcome::Unknown(UnknownCause::NotAdmitted);
    let draft = PhoneOutcome::Draft;
    let order = [
        done,
        running,
        accepted,
        failed,
        unresolved,
        attempted,
        lookup_failed,
        not_admitted,
        draft,
    ];
    for (incoming_at, incoming) in order.iter().enumerate() {
        for (stored_at, stored) in order.iter().enumerate() {
            assert_eq!(
                replaces(*incoming, Evidence::Direct, *stored, Evidence::Direct),
                incoming_at < stored_at,
                "direct {incoming:?} vs direct {stored:?}"
            );
            assert_eq!(
                replaces(*incoming, Evidence::Lookup, *stored, Evidence::Lookup),
                incoming_at < stored_at,
                "lookup {incoming:?} vs lookup {stored:?}"
            );
        }
    }
    let direct_answers = [done, running, accepted, failed, unresolved];
    for incoming in direct_answers {
        for stored in order {
            assert!(
                replaces(incoming, Evidence::Direct, stored, Evidence::Lookup),
                "direct {incoming:?} must replace lookup {stored:?}"
            );
            assert!(
                !replaces(incoming, Evidence::Lookup, stored, Evidence::Direct),
                "lookup {incoming:?} must not replace direct {stored:?}"
            );
        }
    }

    let record = departed("hello");
    for answer in [
        LookupAnswer::Found {
            stage: ReceiptStage::Ready,
            effect: None,
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: None,
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Withdrawn),
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Cancelled),
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::AlreadyFinal),
        },
        LookupAnswer::Found {
            stage: ReceiptStage::Accepted,
            effect: Some(ReceiptEffect::Dispatched),
        },
    ] {
        let step = after_lookup(record.clone(), answer, false);
        assert!(
            matches!(step, super::Step::Done(ref saved) if saved == &record),
            "impossible receipt changed the row: {answer:?} -> {step:?}"
        );
    }
    assert!(submit_outcome(ReceiptStage::Ready, None).is_none());
    assert_eq!(
        submit_outcome(ReceiptStage::Settled, Some(ReceiptEffect::NoEffect)),
        Some(PhoneOutcome::Failed(PhoneFailure::Conversation(
            ConversationErrorCode::Revoked
        )))
    );
    assert_eq!(
        submit_outcome(ReceiptStage::Attempted, None),
        Some(PhoneOutcome::Unknown(UnknownCause::Attempted))
    );

    let not_admitted_row = record.clone();
    let direct = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendAnswered, accepted),
    ];
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(1)
            .with_outcome(accepted),
        &direct,
        false
    ));
    assert_eq!(stored_evidence(&direct), Evidence::Direct);

    let lookup_done = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit_by(Cause::Lookup, done, Initiator::Recovery),
    ];
    assert!(history_reaches(
        &not_admitted_row.clone().with_outcome(done),
        &lookup_done,
        false
    ));
    assert_eq!(stored_evidence(&lookup_done), Evidence::Lookup);

    let resend = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::ForegroundSend, accepted),
    ];
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(2)
            .with_outcome(accepted),
        &resend,
        false
    ));
    let resumed = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::Lookup, lookup_failed),
        audit(Cause::SendLost, lookup_failed),
        audit(Cause::Lookup, not_admitted),
        audit(Cause::ForegroundSend, accepted),
    ];
    assert!(
        history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(3)
                .with_outcome(accepted),
            &resumed,
            false
        ),
        "a lost lookup that finds nothing is a foreground send"
    );
    let forged = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::Lookup, not_admitted),
        audit(Cause::ForegroundSend, accepted),
    ];
    assert!(
        !history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(1)
                .with_outcome(accepted),
            &forged,
            false
        ),
        "foreground send does not follow an explicit-retry lookup"
    );

    let explicit = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::Lookup, not_admitted),
        audit(Cause::ExplicitRetry, not_admitted),
        audit(Cause::ExplicitRetry, accepted),
    ];
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(1)
            .with_outcome(accepted),
        &explicit,
        false
    ));
    assert_eq!(stored_evidence(&explicit), Evidence::Direct);
    assert_eq!(
        stored_evidence(&explicit[..4]),
        Evidence::Lookup,
        "the reset is not a direct answer"
    );

    let running_failure = PhoneOutcome::Failed(PhoneFailure::Conversation(
        ConversationErrorCode::TurnRunning,
    ));
    let successor = command("command-2");
    let mut direct_return = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::ReturnedToDraft, running_failure),
    ];
    direct_return[2].successor = Some(successor.clone());
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(1)
            .with_outcome(running_failure)
            .with_successor(successor.clone()),
        &direct_return,
        false
    ));
    assert!(
        !history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(1)
                .with_outcome(running_failure)
                .with_successor(command("command-1")),
            &direct_return,
            false
        ),
        "a successor that names its own command does not reach"
    );
    assert_eq!(stored_evidence(&direct_return), Evidence::Direct);

    let mut lookup_return = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit_by(Cause::Lookup, running_failure, Initiator::Recovery),
        audit_by(Cause::ReturnedToDraft, running_failure, Initiator::Recovery),
    ];
    lookup_return[3].successor = Some(successor.clone());
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_outcome(running_failure)
            .with_successor(successor.clone()),
        &lookup_return,
        false
    ));
    assert_eq!(stored_evidence(&lookup_return), Evidence::Lookup);
    let mut split = lookup_return.to_vec();
    split[3].initiator = Initiator::Person;
    assert!(
        !history_reaches(
            &not_admitted_row
                .clone()
                .with_outcome(running_failure)
                .with_successor(successor.clone()),
            &split,
            false
        ),
        "one caller writes the lookup and the return"
    );
    let mut recovery_direct = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit_by(Cause::ReturnedToDraft, running_failure, Initiator::Recovery),
    ];
    recovery_direct[2].successor = Some(successor.clone());
    assert!(
        !history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(1)
                .with_outcome(running_failure)
                .with_successor(successor.clone()),
            &recovery_direct,
            false
        ),
        "a direct return is the person"
    );
    let minted = PhoneRecord::held(intent("hello"));
    assert!(history_reaches(
        &minted,
        &[audit_by(Cause::Held, draft, Initiator::Recovery)],
        true
    ));
    assert!(!history_reaches(
        &minted,
        &[audit_by(Cause::Held, draft, Initiator::Recovery)],
        false
    ));

    let over_done = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::Lookup, done),
        audit(Cause::SendAnswered, accepted),
    ];
    assert!(
        history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(1)
                .with_outcome(accepted),
            &over_done,
            false
        ),
        "a direct answer replays from the row before the lookup"
    );
    let mut over_running = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit_by(Cause::Lookup, running_failure, Initiator::Recovery),
        audit_by(Cause::ReturnedToDraft, running_failure, Initiator::Recovery),
        audit(Cause::SendAnswered, accepted),
    ];
    over_running[3].successor = Some(successor.clone());
    assert!(
        history_reaches(
            &not_admitted_row
                .clone()
                .with_sends(1)
                .with_outcome(accepted),
            &over_running,
            false
        ),
        "a direct answer replays from the row before a turn_running lookup"
    );

    let direct_running = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendAnswered, running),
    ];
    assert!(history_reaches(
        &not_admitted_row.clone().with_sends(1).with_outcome(running),
        &direct_running,
        false
    ));
    let lookup_running = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::Lookup, running),
    ];
    assert!(
        !history_reaches(
            &not_admitted_row.clone().with_outcome(running),
            &lookup_running,
            false
        ),
        "a lookup cannot record running"
    );

    let spent_lookup = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::Lookup, lookup_failed),
    ];
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(FOREGROUND_SENDS)
            .with_outcome(lookup_failed),
        &spent_lookup,
        false
    ));
    let mut rewritten = spent_lookup.to_vec();
    rewritten.push(audit(Cause::Bound, not_admitted));
    assert!(
        !history_reaches(
            &not_admitted_row.clone().with_outcome(not_admitted),
            &rewritten,
            false
        ),
        "bound must not replace a failed lookup"
    );
    let bound = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::SendLost, not_admitted),
        audit(Cause::Bound, not_admitted),
    ];
    assert!(history_reaches(
        &not_admitted_row
            .clone()
            .with_sends(FOREGROUND_SENDS)
            .with_outcome(not_admitted),
        &bound,
        false
    ));

    let attempted_prefix = [
        audit(Cause::Held, draft),
        audit(Cause::Departed, not_admitted),
        audit(Cause::Lookup, attempted),
    ];
    assert!(history_reaches(
        &not_admitted_row.clone().with_outcome(attempted),
        &attempted_prefix,
        false
    ));
    for downgrade in [not_admitted, lookup_failed] {
        let mut events = attempted_prefix.to_vec();
        events.push(audit(Cause::Lookup, downgrade));
        assert!(
            !history_reaches(
                &not_admitted_row.clone().with_outcome(downgrade),
                &events,
                false
            ),
            "attempted must not restore as {downgrade:?}"
        );
    }

    let skipped = [
        audit(Cause::Held, draft),
        audit(Cause::SendAnswered, accepted),
    ];
    assert!(!history_reaches(
        &not_admitted_row.clone().with_outcome(accepted),
        &skipped,
        false
    ));
    for cause in [
        Cause::Departed,
        Cause::SendAnswered,
        Cause::SendLost,
        Cause::Unreadable,
        Cause::ForegroundSend,
        Cause::ExplicitRetry,
        Cause::Bound,
    ] {
        let outcome = if cause == Cause::Departed
            || cause == Cause::Bound
            || cause == Cause::SendLost
            || cause == Cause::Unreadable
        {
            not_admitted
        } else {
            accepted
        };
        let mut events = vec![
            audit(Cause::Held, draft),
            audit(Cause::Departed, not_admitted),
        ];
        if cause != Cause::Departed {
            events.push(audit_by(cause, outcome, Initiator::Recovery));
        } else {
            events[1].initiator = Initiator::Recovery;
        }
        let row = not_admitted_row.clone().with_outcome(outcome);
        assert!(
            !history_reaches(&row, &events, false),
            "recovery must not record {cause:?}"
        );
    }
}

fn audit(cause: Cause, outcome: PhoneOutcome) -> IntentEvent {
    audit_by(cause, outcome, Initiator::Person)
}

fn audit_by(cause: Cause, outcome: PhoneOutcome, initiator: Initiator) -> IntentEvent {
    IntentEvent {
        cause,
        outcome,
        initiator,
        observed_at_ms: 1,
        successor: None,
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
