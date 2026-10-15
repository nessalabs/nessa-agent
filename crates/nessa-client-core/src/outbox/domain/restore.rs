//! Whether a stored row is one this outbox could have written, and which
//! answer wins when two of them meet.
//!
//! Restoration rejects a snapshot whose audit rows do not reach it
//! (`a_done_row_whose_history_stops_at_departure_is_corrupt`). It does not
//! repair the row. Precedence is the table in `docs/design/phone-outbox.md`:
//! a direct answer beats any lookup, and inside one source a higher outcome
//! beats a lower one (`every_transition_and_precedence_pair`).

use super::{
    Cause, Initiator, IntentEvent, PhoneFailure, PhoneIntent, PhoneOutcome, PhoneRecord,
    UnknownCause,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// Where an outcome came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// `conversation.send` returned this outcome.
    Direct,
    /// A lookup, a departure, or another row that has not stored a direct answer.
    Lookup,
}

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
    if record.outcome() != events[events.len() - 1].outcome {
        return false;
    }
    let mut state = first.outcome;
    let mut previous = first.cause;
    for event in rest {
        if !initiator_allowed(event.cause, event.initiator) || !step_allowed(state, event, previous)
        {
            return false;
        }
        state = event.outcome;
        previous = event.cause;
    }
    successor_follows(record, events)
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
    let returned = events
        .last()
        .is_some_and(|event| event.cause == Cause::ReturnedToDraft);
    let running = turn_running_outcome(record.outcome());
    matches!(
        (returned, record.successor().is_some(), running),
        (true, true, true) | (false, false, false)
    )
}

fn step_allowed(from: PhoneOutcome, event: &IntentEvent, previous: Cause) -> bool {
    let to = event.outcome;
    match event.cause {
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
            let failed = turn_running_outcome(to);
            (open_unknown(from) && failed) || (from == to && failed && previous == Cause::Lookup)
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
    !matches!(outcome, PhoneOutcome::Draft)
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
