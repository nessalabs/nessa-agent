//! Whether a stored row is one this outbox could have written, and which
//! answer wins when two of them meet.
//!
//! Restoration rejects a snapshot whose audit rows do not reach it
//! (`a_done_row_whose_history_stops_at_departure_is_corrupt`). It does not
//! repair the row. Precedence is the table in `docs/design/phone-outbox.md`:
//! a direct answer beats any lookup, and inside one source a higher outcome
//! beats a lower one (`every_transition_and_precedence_pair`).

use super::decide::{apply_lookup, apply_retry, apply_send, budget, Applied, Budget, Evidence};
use super::{
    Cause, Delivery, Initiator, IntentEvent, LookupAnswer, PhoneFailure, PhoneIntent, PhoneOutcome,
    PhoneRecord, ReceiptEffect, ReceiptStage, SendAnswer, UnknownCause,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// Whether `incoming` should replace `stored`.
///
/// A direct answer replaces any lookup result. A lookup never replaces a
/// stored direct answer. Inside one source, the higher outcome replaces the
/// lower one (`every_transition_and_precedence_pair`).
pub(crate) fn replaces(
    incoming: PhoneOutcome,
    incoming_evidence: Evidence,
    stored: PhoneOutcome,
    stored_evidence: Evidence,
) -> bool {
    match (incoming_evidence, stored_evidence) {
        (Evidence::Direct, Evidence::Lookup) => true,
        (Evidence::Lookup, Evidence::Direct) => false,
        (Evidence::Direct, Evidence::Direct) | (Evidence::Lookup, Evidence::Lookup) => {
            rank(incoming) > rank(stored)
        }
    }
}

/// The evidence of the row these events reach. An empty history is not a
/// direct answer.
pub(crate) fn stored_evidence(events: &[IntentEvent]) -> Evidence {
    let Some(last) = events.last() else {
        return Evidence::Lookup;
    };
    match last.cause {
        Cause::SendAnswered | Cause::ForegroundSend => Evidence::Direct,
        Cause::ExplicitRetry
            if !matches!(
                last.outcome,
                PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
            ) =>
        {
            Evidence::Direct
        }
        Cause::ReturnedToDraft => {
            let lookup_answer = events.len() >= 2
                && events[events.len() - 2].cause == Cause::Lookup
                && events[events.len() - 2].outcome == last.outcome;
            if lookup_answer {
                Evidence::Lookup
            } else {
                Evidence::Direct
            }
        }
        Cause::Held
        | Cause::Departed
        | Cause::SendLost
        | Cause::Unreadable
        | Cause::Lookup
        | Cause::ExplicitRetry
        | Cause::Bound => Evidence::Lookup,
    }
}

/// The events are a legal path from the held draft to `record`'s outcome.
///
/// `recovery_hold` is true only for a draft this recovery minted. The root
/// command's hold is the person's.
pub(crate) fn history_reaches(
    record: &PhoneRecord,
    events: &[IntentEvent],
    recovery_hold: bool,
) -> bool {
    let Some((first, rest)) = events.split_first() else {
        return false;
    };
    if first.cause != Cause::Held || first.outcome != PhoneOutcome::Draft {
        return false;
    }
    if !hold_initiator(first.initiator, recovery_hold) {
        return false;
    }
    let mut state = PhoneRecord::held(record.intent().clone());
    let mut older = first.cause;
    let mut previous = first;
    for event in rest {
        let Some(next) = advance(&state, event, previous, older) else {
            return false;
        };
        state = next;
        older = previous.cause;
        previous = event;
    }
    state.outcome() == record.outcome()
        && state.sends() == record.sends()
        && state.departed() == record.departed()
        && successor_follows(record, events)
}

/// The next row, when `event` is one [`apply_send`], [`apply_lookup`],
/// [`apply_retry`], or [`budget`] would persist. Anything else is not a replay
/// (`an_attempted_receipt_is_not_restored_as_not_admitted`).
fn advance(
    before: &PhoneRecord,
    event: &IntentEvent,
    previous: &IntentEvent,
    older: Cause,
) -> Option<PhoneRecord> {
    if !initiator_allowed(event.cause, event.initiator) {
        return None;
    }
    match event.cause {
        Cause::Held => None,
        Cause::Departed => {
            let next = before.clone().depart()?;
            (next.outcome() == event.outcome).then_some(next)
        }
        Cause::SendLost => marked(before, SendAnswer::Lost, event),
        Cause::Unreadable => marked(before, SendAnswer::Broken, event),
        Cause::SendAnswered => terminal(before, event, Cause::SendAnswered),
        Cause::ForegroundSend => {
            // The same call's next send. Not found that leaves the outcome
            // writes no lookup, so the predecessor is SendLost. Not found
            // that moves lookup failed to not admitted writes Lookup first.
            let after_lost = previous.cause == Cause::SendLost
                || (previous.cause == Cause::Lookup
                    && older == Cause::SendLost
                    && matches!(
                        before.outcome(),
                        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
                    ));
            if !after_lost {
                return None;
            }
            terminal(before, event, Cause::ForegroundSend)
        }
        Cause::ExplicitRetry => explicit(before, event, previous.cause),
        Cause::Lookup => lookup_event(before, event, previous.cause),
        Cause::Bound => bound(before, event, previous.cause),
        Cause::ReturnedToDraft => returned(before, event, previous),
    }
}

fn marked(before: &PhoneRecord, answer: SendAnswer, event: &IntentEvent) -> Option<PhoneRecord> {
    match apply_send(before.clone(), answer, Cause::SendAnswered) {
        Applied::Mark {
            record, cause: got, ..
        } if got == event.cause && record.outcome() == event.outcome => Some(record),
        _ => None,
    }
}

fn terminal(before: &PhoneRecord, event: &IntentEvent, cause: Cause) -> Option<PhoneRecord> {
    let answer = send_witness(event.outcome)?;
    match apply_send(before.clone(), answer, cause) {
        Applied::Save {
            record, cause: got, ..
        } if got == cause && record.outcome() == event.outcome => Some(record),
        _ => None,
    }
}

fn explicit(before: &PhoneRecord, event: &IntentEvent, previous: Cause) -> Option<PhoneRecord> {
    if event.outcome == PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        && previous == Cause::Lookup
    {
        let reset = apply_retry(before.clone())?;
        return (reset.reset.outcome() == event.outcome).then_some(reset.reset);
    }
    if previous == Cause::ExplicitRetry {
        return terminal(before, event, Cause::ExplicitRetry);
    }
    None
}

fn lookup_event(before: &PhoneRecord, event: &IntentEvent, previous: Cause) -> Option<PhoneRecord> {
    let resend = previous == Cause::SendLost;
    for answer in lookup_witnesses(event.outcome) {
        if let Some(record) = lookup_applied(before, answer, false, event.outcome) {
            return Some(record);
        }
        if resend {
            if let Some(record) = lookup_applied(before, answer, true, event.outcome) {
                return Some(record);
            }
        }
    }
    if event.outcome == PhoneOutcome::Unknown(UnknownCause::NotAdmitted) {
        let reset = apply_retry(before.clone())?;
        if reset.looked.outcome() == event.outcome && reset.looked.sends() == before.sends() {
            return Some(reset.looked);
        }
    }
    None
}

fn lookup_applied(
    before: &PhoneRecord,
    answer: LookupAnswer,
    resend: bool,
    outcome: PhoneOutcome,
) -> Option<PhoneRecord> {
    match apply_lookup(before.clone(), answer, resend) {
        Applied::Save {
            record,
            cause: Cause::Lookup,
            ..
        } if record.outcome() == outcome => Some(record),
        Applied::Resend { record }
            if resend && record.outcome() == outcome && record.outcome() != before.outcome() =>
        {
            Some(record)
        }
        Applied::Return {
            record,
            lookup: true,
        } if record.outcome() == outcome => Some(record),
        _ => None,
    }
}

fn bound(before: &PhoneRecord, event: &IntentEvent, previous: Cause) -> Option<PhoneRecord> {
    if event.outcome != PhoneOutcome::Unknown(UnknownCause::NotAdmitted) {
        return None;
    }
    if let Budget::Bound(record) = budget(before.clone()) {
        if record.outcome() == event.outcome {
            return Some(record);
        }
    }
    if previous == Cause::SendLost {
        if let Applied::Save {
            record,
            cause: Cause::Bound,
            ..
        } = apply_lookup(before.clone(), LookupAnswer::NotFound, true)
        {
            if record.outcome() == event.outcome {
                return Some(record);
            }
        }
    }
    None
}

fn returned(
    before: &PhoneRecord,
    event: &IntentEvent,
    previous: &IntentEvent,
) -> Option<PhoneRecord> {
    if !turn_running_outcome(event.outcome) {
        return None;
    }
    // One caller writes the lookup and the return, and the direct send is the person.
    if previous.cause == Cause::Lookup && before.outcome() == event.outcome {
        return (previous.initiator == event.initiator).then_some(before.clone());
    }
    if event.initiator != Initiator::Person {
        return None;
    }
    let answer = SendAnswer::Refused(PhoneFailure::Conversation(
        ConversationErrorCode::TurnRunning,
    ));
    match apply_send(before.clone(), answer, Cause::SendAnswered) {
        Applied::Return {
            record,
            lookup: false,
        } if record.outcome() == event.outcome => Some(record),
        _ => None,
    }
}

fn send_witness(outcome: PhoneOutcome) -> Option<SendAnswer> {
    Some(match outcome {
        PhoneOutcome::Accepted => SendAnswer::Receipt(Delivery::Queued),
        PhoneOutcome::Running => SendAnswer::Receipt(Delivery::Injected),
        PhoneOutcome::Done => SendAnswer::Receipt(Delivery::Settled),
        PhoneOutcome::Unknown(UnknownCause::Unresolved) => SendAnswer::Refused(
            PhoneFailure::Conversation(ConversationErrorCode::SubmissionUnresolved),
        ),
        PhoneOutcome::Failed(failure) => SendAnswer::Refused(failure),
        PhoneOutcome::Draft | PhoneOutcome::Unknown(_) => return None,
    })
}

fn lookup_witnesses(outcome: PhoneOutcome) -> Vec<LookupAnswer> {
    let mut answers = Vec::new();
    match outcome {
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted) => answers.push(LookupAnswer::NotFound),
        PhoneOutcome::Unknown(UnknownCause::LookupFailed) => {
            answers.push(LookupAnswer::Lost);
            answers.push(LookupAnswer::Broken);
        }
        PhoneOutcome::Unknown(UnknownCause::Unresolved) => answers.push(LookupAnswer::Refused(
            PhoneFailure::Conversation(ConversationErrorCode::SubmissionUnresolved),
        )),
        PhoneOutcome::Unknown(UnknownCause::Attempted) => answers.push(LookupAnswer::Found {
            stage: ReceiptStage::Attempted,
            effect: None,
        }),
        PhoneOutcome::Accepted => answers.push(LookupAnswer::Found {
            stage: ReceiptStage::Accepted,
            effect: None,
        }),
        PhoneOutcome::Done => answers.push(LookupAnswer::Found {
            stage: ReceiptStage::Settled,
            effect: Some(ReceiptEffect::Dispatched),
        }),
        PhoneOutcome::Running | PhoneOutcome::Draft => {}
        PhoneOutcome::Failed(failure) => {
            answers.push(LookupAnswer::Refused(failure));
            if let PhoneFailure::Conversation(code) = failure {
                let effect = match code {
                    ConversationErrorCode::Revoked => Some(ReceiptEffect::NoEffect),
                    ConversationErrorCode::RevokedAfterSave => {
                        Some(ReceiptEffect::RefusedAfterSave)
                    }
                    ConversationErrorCode::UnavailableAfterSave => {
                        Some(ReceiptEffect::UnavailableAfterSave)
                    }
                    ConversationErrorCode::SubmissionConflict => {
                        answers.push(LookupAnswer::Conflict);
                        None
                    }
                    _ => None,
                };
                if let Some(effect) = effect {
                    answers.push(LookupAnswer::Found {
                        stage: ReceiptStage::Settled,
                        effect: Some(effect),
                    });
                }
            }
        }
    }
    answers
}

/// The new draft carries the text and images `turn_running` handed back.
pub(crate) fn same_prompt(failed: &PhoneIntent, draft: &PhoneIntent) -> bool {
    failed.conversation() == draft.conversation()
        && failed.text() == draft.text()
        && failed.images() == draft.images()
}

fn hold_initiator(initiator: Initiator, recovery_hold: bool) -> bool {
    match initiator {
        Initiator::Person => true,
        Initiator::Recovery => recovery_hold,
    }
}

fn initiator_allowed(cause: Cause, initiator: Initiator) -> bool {
    match cause {
        Cause::Departed
        | Cause::SendAnswered
        | Cause::SendLost
        | Cause::Unreadable
        | Cause::ForegroundSend
        | Cause::ExplicitRetry
        | Cause::Bound => initiator == Initiator::Person,
        Cause::Lookup | Cause::ReturnedToDraft | Cause::Held => true,
    }
}

fn successor_follows(record: &PhoneRecord, events: &[IntentEvent]) -> bool {
    if record
        .successor()
        .is_some_and(|successor| successor.as_str() == record.intent().command().as_str())
    {
        return false;
    }
    let returned = events
        .last()
        .is_some_and(|event| event.cause == Cause::ReturnedToDraft);
    let running = turn_running_outcome(record.outcome());
    matches!(
        (returned, record.successor().is_some(), running),
        (true, true, true) | (false, false, false)
    )
}

fn turn_running_outcome(outcome: PhoneOutcome) -> bool {
    matches!(outcome, PhoneOutcome::Failed(failure) if turn_running(failure))
}

fn turn_running(failure: PhoneFailure) -> bool {
    matches!(
        failure,
        PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)
    )
}

/// Highest first: done, running, accepted, failed, unresolved, attempted,
/// lookup failed, not admitted, draft.
fn rank(outcome: PhoneOutcome) -> u8 {
    match outcome {
        PhoneOutcome::Done => 8,
        PhoneOutcome::Running => 7,
        PhoneOutcome::Accepted => 6,
        PhoneOutcome::Failed(_) => 5,
        PhoneOutcome::Unknown(UnknownCause::Unresolved) => 4,
        PhoneOutcome::Unknown(UnknownCause::Attempted) => 3,
        PhoneOutcome::Unknown(UnknownCause::LookupFailed) => 2,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted) => 1,
        PhoneOutcome::Draft => 0,
    }
}
