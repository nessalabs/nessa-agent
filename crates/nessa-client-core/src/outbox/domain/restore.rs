//! Whether a stored row is one this outbox could have written.
//!
//! Restoration rejects a snapshot whose audit rows do not reach it
//! (`a_done_row_whose_history_stops_at_departure_is_corrupt`). It does not
//! repair the row.

use super::{
    Cause, IntentEvent, PhoneFailure, PhoneIntent, PhoneOutcome, PhoneRecord, UnknownCause,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// `candidate` is evidence the committed row does not yet have.
///
/// A queued, injected, or settled receipt outranks a lookup that failed or
/// found nothing (`a_queued_receipt_replaces_a_weaker_lookup_failure`).
/// A committed done or failed row is not replaced by an earlier receipt.
pub(crate) fn outranks(candidate: PhoneOutcome, committed: PhoneOutcome) -> bool {
    rank(candidate) > rank(committed)
}

/// The events are a legal path from the held draft to `record`'s outcome.
pub(crate) fn history_reaches(record: &PhoneRecord, events: &[IntentEvent]) -> bool {
    let Some((first, rest)) = events.split_first() else {
        return false;
    };
    if first.cause != Cause::Held || first.outcome != PhoneOutcome::Draft {
        return false;
    }
    if record.outcome() != events[events.len() - 1].outcome {
        return false;
    }
    let mut state = first.outcome;
    for event in rest {
        if !step_allowed(state, event.cause, event.outcome) {
            return false;
        }
        state = event.outcome;
    }
    successor_follows(record, events)
}

/// The new draft carries the text and images `turn_running` handed back.
pub(crate) fn same_prompt(failed: &PhoneIntent, draft: &PhoneIntent) -> bool {
    failed.conversation() == draft.conversation()
        && failed.text() == draft.text()
        && failed.images() == draft.images()
}

fn successor_follows(record: &PhoneRecord, events: &[IntentEvent]) -> bool {
    let returned = events
        .last()
        .is_some_and(|event| event.cause == Cause::ReturnedToDraft);
    match (returned, record.successor().is_some()) {
        (true, true) => matches!(
            record.outcome(),
            PhoneOutcome::Failed(PhoneFailure::Conversation(
                ConversationErrorCode::TurnRunning
            ))
        ),
        (false, false) => true,
        _ => false,
    }
}

fn step_allowed(from: PhoneOutcome, cause: Cause, to: PhoneOutcome) -> bool {
    match cause {
        Cause::Held => false,
        Cause::Departed => {
            from == PhoneOutcome::Draft && to == PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        }
        Cause::SendLost | Cause::Unreadable | Cause::Bound => {
            from == PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
                && to == PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        }
        Cause::SendAnswered | Cause::ForegroundSend => open_unknown(from) && send_outcome(to),
        Cause::ExplicitRetry => {
            (from == PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
                && to == PhoneOutcome::Unknown(UnknownCause::NotAdmitted))
                || (open_unknown(from) && send_outcome(to))
        }
        Cause::Lookup => open_unknown(from) && lookup_outcome(to),
        Cause::ReturnedToDraft => {
            open_unknown(from)
                && matches!(
                    to,
                    PhoneOutcome::Failed(PhoneFailure::Conversation(
                        ConversationErrorCode::TurnRunning
                    ))
                )
        }
    }
}

fn open_unknown(outcome: PhoneOutcome) -> bool {
    matches!(
        outcome,
        PhoneOutcome::Unknown(
            UnknownCause::NotAdmitted | UnknownCause::LookupFailed | UnknownCause::Attempted
        )
    )
}

fn send_outcome(outcome: PhoneOutcome) -> bool {
    match outcome {
        PhoneOutcome::Accepted | PhoneOutcome::Running | PhoneOutcome::Done => true,
        PhoneOutcome::Unknown(UnknownCause::Unresolved) => true,
        PhoneOutcome::Failed(failure) => !turn_running(failure),
        PhoneOutcome::Draft | PhoneOutcome::Unknown(_) => false,
    }
}

fn lookup_outcome(outcome: PhoneOutcome) -> bool {
    match outcome {
        PhoneOutcome::Draft => false,
        PhoneOutcome::Failed(failure) if turn_running(failure) => false,
        _ => true,
    }
}

fn turn_running(failure: PhoneFailure) -> bool {
    matches!(
        failure,
        PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)
    )
}

fn rank(outcome: PhoneOutcome) -> u8 {
    match outcome {
        PhoneOutcome::Draft => 0,
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted | UnknownCause::LookupFailed) => 1,
        PhoneOutcome::Unknown(UnknownCause::Attempted) => 2,
        PhoneOutcome::Unknown(UnknownCause::Unresolved) => 3,
        PhoneOutcome::Accepted => 4,
        PhoneOutcome::Running => 5,
        PhoneOutcome::Done | PhoneOutcome::Failed(_) => 6,
    }
}
