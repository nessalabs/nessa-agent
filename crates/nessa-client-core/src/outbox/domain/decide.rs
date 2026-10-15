//! How a gateway answer moves a departed command. No store and no socket.

use super::{
    Cause, Delivery, LookupAnswer, PhoneFailure, PhoneOutcome, PhoneRecord, ReceiptEffect,
    ReceiptStage, SendAnswer, UnknownCause,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;

/// Sends one foreground call may make, the first included.
///
/// `foreground_send_stops_at_the_bound` counts them. A restart does not
/// continue the count into another send; it reads the receipt.
pub const FOREGROUND_SENDS: u8 = 3;

/// The next thing the outbox does with a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Save this record and stop.
    Done(PhoneRecord),
    /// Save this record, then read the receipt. `resend` is true only when the
    /// send answer did not arrive, so an unreadable answer is not sent again.
    Lookup { record: PhoneRecord, resend: bool },
    /// Save this record, then send the same bytes again.
    Send(PhoneRecord),
    /// Save this failed record and put its text on a new draft.
    ReturnToDraft(PhoneRecord),
}

/// Apply one `conversation.send` answer. A lost or unreadable answer looks
/// the receipt up; it does not by itself send again.
pub(crate) fn after_send(record: PhoneRecord, answer: SendAnswer) -> Step {
    let sends = record.sends().saturating_add(1);
    let record = record.with_sends(sends);
    match answer {
        SendAnswer::Receipt(Delivery::Queued) => {
            Step::Done(record.with_outcome(PhoneOutcome::Accepted))
        }
        SendAnswer::Receipt(Delivery::Injected) => {
            Step::Done(record.with_outcome(PhoneOutcome::Running))
        }
        SendAnswer::Receipt(Delivery::Settled) => {
            Step::Done(record.with_outcome(PhoneOutcome::Done))
        }
        SendAnswer::Refused(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionUnresolved,
        )) => Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::Unresolved))),
        SendAnswer::Refused(PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)) => {
            Step::ReturnToDraft(record.with_outcome(PhoneOutcome::Failed(
                PhoneFailure::Conversation(ConversationErrorCode::TurnRunning),
            )))
        }
        SendAnswer::Refused(failure) => {
            Step::Done(record.with_outcome(PhoneOutcome::Failed(failure)))
        }
        SendAnswer::Lost => Step::Lookup {
            record,
            resend: true,
        },
        SendAnswer::Broken => Step::Lookup {
            record,
            resend: false,
        },
    }
}

/// Apply one `conversation.receipt` answer.
///
/// `allow_send` is true only inside the foreground call that already sent
/// and found nothing. A restart passes false, so an absent receipt stays
/// unknown (`recovery_does_not_send_when_the_receipt_is_absent`).
pub(crate) fn after_lookup(record: PhoneRecord, answer: LookupAnswer, allow_send: bool) -> Step {
    // An attempted receipt is already evidence of a submit. A restart that
    // cannot see a newer receipt leaves it attempted, so a later recovery
    // can still adopt a settled one. It is not downgraded to not-admitted.
    if !allow_send
        && matches!(
            record.outcome(),
            PhoneOutcome::Unknown(UnknownCause::Attempted)
        )
        && matches!(
            answer,
            LookupAnswer::NotFound | LookupAnswer::Lost | LookupAnswer::Broken
        )
    {
        return Step::Done(record);
    }
    match answer {
        LookupAnswer::Lost | LookupAnswer::Broken => {
            Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::LookupFailed)))
        }
        LookupAnswer::NotFound => {
            let record = record.with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
            if allow_send && record.sends() < FOREGROUND_SENDS {
                Step::Send(record)
            } else {
                Step::Done(record)
            }
        }
        LookupAnswer::Conflict => Step::Done(record.with_outcome(PhoneOutcome::Failed(
            PhoneFailure::Conversation(ConversationErrorCode::SubmissionConflict),
        ))),
        LookupAnswer::Refused(PhoneFailure::Conversation(
            ConversationErrorCode::SubmissionUnresolved,
        )) => Step::Done(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::Unresolved))),
        LookupAnswer::Refused(PhoneFailure::Conversation(ConversationErrorCode::TurnRunning)) => {
            Step::ReturnToDraft(record.with_outcome(PhoneOutcome::Failed(
                PhoneFailure::Conversation(ConversationErrorCode::TurnRunning),
            )))
        }
        LookupAnswer::Refused(failure) => {
            Step::Done(record.with_outcome(PhoneOutcome::Failed(failure)))
        }
        LookupAnswer::Found { stage, effect } => match submit_outcome(stage, effect) {
            Some(outcome) => Step::Done(record.with_outcome(outcome)),
            None => Step::Done(record),
        },
    }
}

/// The outcome a submit receipt stores.
///
/// `None` is not an outcome: stage `ready`, an effect on a stage that has not
/// settled, a settled receipt that omits its effect, or a settled stop effect
/// (`withdrawn`, `cancelled`, `already_final`). The row stays. `no_effect` is
/// failed (`a_settled_receipt_becomes_done_or_the_matching_refusal`).
pub(crate) fn submit_outcome(
    stage: ReceiptStage,
    effect: Option<ReceiptEffect>,
) -> Option<PhoneOutcome> {
    match stage {
        ReceiptStage::Accepted if effect.is_none() => Some(PhoneOutcome::Accepted),
        ReceiptStage::Attempted if effect.is_none() => {
            Some(PhoneOutcome::Unknown(UnknownCause::Attempted))
        }
        ReceiptStage::Settled => match effect {
            Some(ReceiptEffect::Dispatched) => Some(PhoneOutcome::Done),
            Some(ReceiptEffect::NoEffect) => Some(failed(ConversationErrorCode::Revoked)),
            Some(ReceiptEffect::RefusedAfterSave) => {
                Some(failed(ConversationErrorCode::RevokedAfterSave))
            }
            Some(ReceiptEffect::UnavailableAfterSave) => {
                Some(failed(ConversationErrorCode::UnavailableAfterSave))
            }
            _ => None,
        },
        ReceiptStage::Accepted | ReceiptStage::Attempted | ReceiptStage::Ready => None,
    }
}

fn failed(code: ConversationErrorCode) -> PhoneOutcome {
    PhoneOutcome::Failed(PhoneFailure::Conversation(code))
}

/// Where an outcome came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Evidence {
    /// `conversation.send` returned this outcome.
    Direct,
    /// A lookup, a departure, or another row that has not stored a direct answer.
    Lookup,
}

/// What one live answer persists. The outbox and restoration both ask this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Append one event and adopt `record`.
    Save {
        /// The row after the event.
        record: PhoneRecord,
        /// The audit cause.
        cause: Cause,
        /// Whether the cause is a direct answer.
        evidence: Evidence,
    },
    /// Append SendLost or Unreadable, then look the receipt up.
    Mark {
        /// The row after the lost or unreadable send.
        record: PhoneRecord,
        /// SendLost or Unreadable.
        cause: Cause,
        /// Whether an absent receipt may send again.
        resend: bool,
    },
    /// `turn_running`. `lookup` writes a Lookup event before ReturnedToDraft.
    Return {
        /// The failed row, before its successor is named.
        record: PhoneRecord,
        /// True when the refusal came from a lookup.
        lookup: bool,
    },
    /// Not found while the budget remains. No event. Send `record` again.
    Resend {
        /// The row the next send uses.
        record: PhoneRecord,
    },
    /// The row stays. No event.
    Keep,
}

/// Whether foreground may call `conversation.send`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Budget {
    /// The send count is under the budget and the outcome may still be sent.
    Send,
    /// Not admitted, and the budget is spent. Persist Bound.
    Bound(PhoneRecord),
    /// Write nothing. Lookup failed at the budget is this, and so is a finished row.
    Stop,
}

/// The explicit-retry pair: a not-admitted lookup, then a send count of zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetryReset {
    /// Lookup of not admitted. The send count is unchanged.
    pub looked: PhoneRecord,
    /// ExplicitRetry. The send count is zero.
    pub reset: PhoneRecord,
}

/// Not admitted or lookup failed. Attempted is not retried.
pub(crate) fn retryable(record: &PhoneRecord) -> bool {
    matches!(
        record.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted | UnknownCause::LookupFailed)
    )
}

/// What foreground does before it asks `conversation.send`.
pub(crate) fn budget(record: PhoneRecord) -> Budget {
    if record.sends() < FOREGROUND_SENDS {
        if retryable(&record) {
            Budget::Send
        } else {
            Budget::Stop
        }
    } else if matches!(
        record.outcome(),
        PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
    ) {
        Budget::Bound(record.with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted)))
    } else {
        Budget::Stop
    }
}

/// The event `conversation.send` persists, including the lost and unreadable marks.
///
/// At the budget this persists nothing: the caller asked [`budget`] first
/// (`a_spent_budget_keeps_lookup_failed`).
pub(crate) fn apply_send(record: PhoneRecord, answer: SendAnswer, cause: Cause) -> Applied {
    if !matches!(
        cause,
        Cause::SendAnswered | Cause::ForegroundSend | Cause::ExplicitRetry
    ) || !matches!(budget(record.clone()), Budget::Send)
    {
        return Applied::Keep;
    }
    match after_send(record, answer) {
        Step::Done(next) => Applied::Save {
            record: next,
            cause,
            evidence: Evidence::Direct,
        },
        Step::ReturnToDraft(record) => Applied::Return {
            record,
            lookup: false,
        },
        Step::Lookup { record, resend } => Applied::Mark {
            cause: if resend {
                Cause::SendLost
            } else {
                Cause::Unreadable
            },
            resend,
            record,
        },
        Step::Send(_) => Applied::Keep,
    }
}

/// The event `conversation.receipt` persists.
///
/// `resend` is true only after SendLost in that same foreground call.
/// Not found on attempted leaves the row
/// (`an_attempted_receipt_is_not_restored_as_not_admitted`). Bound is only
/// not admitted staying not admitted.
pub(crate) fn apply_lookup(record: PhoneRecord, answer: LookupAnswer, resend: bool) -> Applied {
    let before = record.clone();
    match after_lookup(record, answer, resend) {
        Step::Done(next) if next == before && !bound_stop(&before, &next, resend) => Applied::Keep,
        Step::Done(next) => Applied::Save {
            cause: if bound_stop(&before, &next, resend) {
                Cause::Bound
            } else {
                Cause::Lookup
            },
            evidence: Evidence::Lookup,
            record: next,
        },
        Step::ReturnToDraft(record) => Applied::Return {
            record,
            lookup: true,
        },
        Step::Send(record) => Applied::Resend { record },
        Step::Lookup { .. } => Applied::Keep,
    }
}

/// The two events an explicit retry writes before it sends.
pub(crate) fn apply_retry(record: PhoneRecord) -> Option<RetryReset> {
    if !retryable(&record) {
        return None;
    }
    let looked = record.with_outcome(PhoneOutcome::Unknown(UnknownCause::NotAdmitted));
    let reset = looked.clone().with_sends(0);
    Some(RetryReset { looked, reset })
}

fn bound_stop(before: &PhoneRecord, next: &PhoneRecord, resend: bool) -> bool {
    resend
        && before.sends() >= FOREGROUND_SENDS
        && matches!(
            before.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        )
        && matches!(
            next.outcome(),
            PhoneOutcome::Unknown(UnknownCause::NotAdmitted)
        )
}
