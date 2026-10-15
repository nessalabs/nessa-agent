//! Whether a stored row is one this outbox could have written, and which
//! answer wins when two of them meet.
//!
//! Restoration rejects a snapshot whose audit rows do not reach it
//! (`a_done_row_whose_history_stops_at_departure_is_corrupt`). It does not
//! repair the row. A direct answer written after the answer it replaces is
//! replayed from the row before that suffix. The suffix is a lookup, or a
//! lower-ranked direct answer (`a_direct_answer_after_a_lookup_still_loads`,
//! `a_stronger_direct_answer_replaces_a_weaker_one`). Precedence is the table
//! in `docs/design/phone-outbox.md`: a direct answer beats any lookup, and
//! inside one source a higher outcome beats a lower one
//! (`every_transition_and_precedence_pair`).

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
    let Some(reached) = fold_reached(record, events, recovery_hold) else {
        return false;
    };
    let Some(state) = reached.last() else {
        return false;
    };
    state.outcome() == record.outcome()
        && state.sends() == record.sends()
        && state.departed() == record.departed()
        && successor_follows(record, events)
}

/// `reached[i]` is the row after `events[i]`.
fn fold_reached(
    record: &PhoneRecord,
    events: &[IntentEvent],
    recovery_hold: bool,
) -> Option<Vec<PhoneRecord>> {
    let (first, rest) = events.split_first()?;
    if first.cause != Cause::Held || first.outcome != PhoneOutcome::Draft {
        return None;
    }
    if !hold_initiator(first.initiator, recovery_hold) {
        return None;
    }
    let mut state = PhoneRecord::held(record.intent().clone());
    let mut older = first.cause;
    let mut previous = first;
    let mut reached = vec![state.clone()];
    for (offset, event) in rest.iter().enumerate() {
        let next = advance(&state, event, previous, older)
            .or_else(|| direct_over_replaced(events, &reached, event))
            .or_else(|| counted_send_before_replaced(events, &reached, offset + 1, event))?;
        state = next;
        older = previous.cause;
        previous = event;
        reached.push(state.clone());
    }
    Some(reached)
}

/// The committed row before the lookup or direct answers `winner` replaces.
///
/// Counted replay applies its steps from here, which is the row
/// [`counted_send_before_replaced`] uses for a prerequisite send
/// (`a_replay_rebases_send_counts_onto_the_committed_history`).
pub(crate) fn before_replaced_direct(
    record: &PhoneRecord,
    events: &[IntentEvent],
    winner: PhoneOutcome,
) -> Option<PhoneRecord> {
    let recovery_hold = events
        .first()
        .is_some_and(|event| event.cause == Cause::Held && event.initiator == Initiator::Recovery);
    let reached = fold_reached(record, events, recovery_hold)?;
    let cut = replaced_suffix_cut(events, reached.len(), winner);
    if cut == 0 || cut == reached.len() {
        return None;
    }
    Some(reached[cut - 1].clone())
}

/// Reconstruct an answer against committed history, including a retry reset.
/// The rolled-back answer's send count is not an input
/// (`a_direct_replay_rebases_after_a_concurrent_retry_reset`,
/// `a_refusal_replay_rebases_after_a_concurrent_retry_reset`).
pub(crate) fn rebase_answer(
    saved: &PhoneRecord,
    events: &[IntentEvent],
    cause: Cause,
    outcome: PhoneOutcome,
    evidence: Evidence,
) -> Option<PhoneRecord> {
    let base = match evidence {
        Evidence::Direct => {
            before_replaced_direct(saved, events, outcome).unwrap_or_else(|| saved.clone())
        }
        Evidence::Lookup => saved.clone(),
    };
    let record = if cause == Cause::ReturnedToDraft {
        counted_return(&base, evidence)?
    } else {
        counted_step(&base, cause, outcome)?
    };
    (record.outcome() == outcome).then_some(record)
}

/// The record `cause` persists when it is applied to `before`.
///
/// Lost and unreadable marks keep `before`'s outcome and increment its send
/// count. A terminal answer uses `outcome`. The rolled-back snapshot's count
/// is not an input. An explicit retry replays its not-found lookup even when
/// unchanged, then resets the count through the same helpers as restoration
/// (`a_replayed_explicit_retry_keeps_its_prefix_after_a_competing_answer`).
pub(crate) fn counted_step(
    before: &PhoneRecord,
    cause: Cause,
    outcome: PhoneOutcome,
) -> Option<PhoneRecord> {
    let event = IntentEvent {
        cause,
        outcome,
        initiator: Initiator::Person,
        observed_at_ms: 0,
        successor: None,
    };
    let record = match cause {
        Cause::SendLost => marked(before, SendAnswer::Lost, &event)?,
        Cause::Unreadable => marked(before, SendAnswer::Broken, &event)?,
        Cause::SendAnswered | Cause::ForegroundSend => terminal(before, &event, cause)
            .or_else(|| observed_turn_running(before, &event, cause))?,
        Cause::ExplicitRetry => explicit(
            before,
            &event,
            if outcome == PhoneOutcome::Unknown(UnknownCause::NotAdmitted) {
                Cause::Lookup
            } else {
                Cause::ExplicitRetry
            },
        )?,
        // The recorded lease may contain a foreground not-found lookup or
        // the explicit retry's lookup, including one that changes no outcome.
        Cause::Lookup => lookup_event(before, &event, Cause::SendLost)?,
        Cause::Held | Cause::Departed | Cause::Bound | Cause::ReturnedToDraft => return None,
    };
    Some(record.clear_successor())
}

/// The failed row a `turn_running` answer persists before its draft is minted.
pub(crate) fn counted_return(before: &PhoneRecord, evidence: Evidence) -> Option<PhoneRecord> {
    let failure = PhoneFailure::Conversation(ConversationErrorCode::TurnRunning);
    let record = match evidence {
        Evidence::Direct => {
            match apply_send(
                before.clone(),
                SendAnswer::Refused(failure),
                Cause::SendAnswered,
            ) {
                Applied::Return {
                    record,
                    lookup: false,
                } => record,
                _ => return None,
            }
        }
        Evidence::Lookup => {
            match apply_lookup(before.clone(), LookupAnswer::Refused(failure), true) {
                Applied::Return {
                    record,
                    lookup: true,
                } => record,
                _ => return None,
            }
        }
    };
    Some(record.clear_successor())
}

/// How far the trailing lookup or weaker direct answers extend.
fn replaced_suffix_cut(events: &[IntentEvent], len: usize, winner: PhoneOutcome) -> usize {
    let mut cut = len;
    while cut > 0
        && (lookup_suffix(&events[cut - 1], events.get(cut.wrapping_sub(2)))
            || (direct_answer_event(&events[cut - 1])
                && replaces(
                    winner,
                    Evidence::Direct,
                    events[cut - 1].outcome,
                    Evidence::Direct,
                )))
    {
        cut -= 1;
    }
    cut
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
        Cause::SendAnswered => observed_turn_running(before, event, Cause::SendAnswered)
            .or_else(|| terminal(before, event, Cause::SendAnswered)),
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
            observed_turn_running(before, event, Cause::ForegroundSend)
                .or_else(|| terminal(before, event, Cause::ForegroundSend))
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
        return observed_turn_running(before, event, Cause::ExplicitRetry)
            .or_else(|| terminal(before, event, Cause::ExplicitRetry));
    }
    None
}

/// `turn_running` observed when the replacement draft was not minted.
///
/// The event keeps the send or retry cause. It has no successor
/// (`a_turn_running_refusal_is_saved_when_the_draft_id_fails`).
fn observed_turn_running(
    before: &PhoneRecord,
    event: &IntentEvent,
    cause: Cause,
) -> Option<PhoneRecord> {
    if !turn_running_outcome(event.outcome) || event.successor.is_some() {
        return None;
    }
    let answer = SendAnswer::Refused(PhoneFailure::Conversation(
        ConversationErrorCode::TurnRunning,
    ));
    match apply_send(before.clone(), answer, cause) {
        Applied::Return {
            record,
            lookup: false,
        } if record.outcome() == event.outcome => Some(record),
        _ => None,
    }
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
    if events.iter().any(|event| {
        event.cause != Cause::ReturnedToDraft && event.successor.is_some()
            || event.cause == Cause::ReturnedToDraft && event.successor.is_none()
    }) {
        return false;
    }
    let Some(last) = events.last() else {
        return false;
    };
    if last.cause == Cause::ReturnedToDraft {
        return turn_running_outcome(record.outcome())
            && last.successor.as_ref() == record.successor();
    }
    if turn_running_outcome(record.outcome()) {
        return record.successor().is_none()
            && turn_running_outcome(last.outcome)
            && matches!(
                last.cause,
                Cause::SendAnswered | Cause::ForegroundSend | Cause::ExplicitRetry | Cause::Lookup
            );
    }
    record.successor().is_none()
}

/// A direct answer appended after the stored answer it replaces.
///
/// [`replaces`] decides the suffix. A lookup suffix loses to any direct
/// answer. A direct suffix loses only when its outcome ranks lower. `apply_send`
/// cannot move a terminal row, so the answer is applied to the row before that
/// suffix. The replaced events stay
/// (`a_direct_answer_after_a_lookup_still_loads`,
/// `a_stronger_direct_answer_replaces_a_weaker_one`).
fn direct_over_replaced(
    events: &[IntentEvent],
    reached: &[PhoneRecord],
    event: &IntentEvent,
) -> Option<PhoneRecord> {
    if !direct_answer_event(event) {
        return None;
    }
    let cut = replaced_suffix_cut(events, reached.len(), event.outcome);
    if cut == reached.len() || cut == 0 {
        return None;
    }
    let base = &reached[cut - 1];
    let previous = &events[cut - 1];
    let older = if cut >= 2 {
        events[cut - 2].cause
    } else {
        previous.cause
    };
    if !initiator_allowed(event.cause, event.initiator) {
        return None;
    }
    match event.cause {
        Cause::ExplicitRetry
            if !matches!(
                event.outcome,
                PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
            ) =>
        {
            terminal(base, event, Cause::ExplicitRetry)
        }
        Cause::SendAnswered | Cause::ForegroundSend | Cause::ReturnedToDraft => {
            advance(base, event, previous, older)
        }
        _ => None,
    }
}

fn direct_answer_event(event: &IntentEvent) -> bool {
    match event.cause {
        Cause::SendAnswered | Cause::ForegroundSend => true,
        Cause::ReturnedToDraft => event.initiator == Initiator::Person,
        Cause::ExplicitRetry => !matches!(
            event.outcome,
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        ),
        Cause::Held
        | Cause::Departed
        | Cause::SendLost
        | Cause::Unreadable
        | Cause::Lookup
        | Cause::Bound => false,
    }
}

/// A lost or unreadable send that a later direct answer still depends on.
///
/// The send does not itself outrank a stored direct answer. It is applied to
/// the row before the lower direct suffix that the later answer replaces, so
/// the fold counts that send
/// (`a_replay_writes_the_lost_send_before_the_stronger_answer`).
/// A lost send with no later outranking answer is not a replay.
fn counted_send_before_replaced(
    events: &[IntentEvent],
    reached: &[PhoneRecord],
    index: usize,
    event: &IntentEvent,
) -> Option<PhoneRecord> {
    if !matches!(event.cause, Cause::SendLost | Cause::Unreadable) {
        return None;
    }
    let later = events
        .get(index + 1..)?
        .iter()
        .find(|candidate| direct_answer_event(candidate))?;
    let cut = replaced_suffix_cut(events, reached.len(), later.outcome);
    if cut == reached.len() || cut == 0 {
        return None;
    }
    let base = &reached[cut - 1];
    let previous = &events[cut - 1];
    let older = if cut >= 2 {
        events[cut - 2].cause
    } else {
        previous.cause
    };
    if !initiator_allowed(event.cause, event.initiator) {
        return None;
    }
    advance(base, event, previous, older)
}

fn lookup_suffix(event: &IntentEvent, previous: Option<&IntentEvent>) -> bool {
    match event.cause {
        Cause::Lookup | Cause::Bound => true,
        Cause::ReturnedToDraft => previous.is_some_and(|previous| {
            previous.cause == Cause::Lookup
                && previous.outcome == event.outcome
                && previous.initiator == event.initiator
        }),
        _ => false,
    }
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
